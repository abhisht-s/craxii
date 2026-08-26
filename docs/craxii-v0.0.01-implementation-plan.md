# Craxii V0.0.01 Implementation Plan

<!-- markdownlint-disable MD013 MD024 -->

**Status:** Authoritative pre-implementation plan  
**Plan revision:** 1  
**Prepared:** 2026-08-26  
**Primary architecture:** `docs/craxii-v0.0.01-architecture.md`  
**Implementation state at preparation:** Documentation only; no application implementation exists

This plan is the chronological implementation source of truth for Craxii V0.0.01. It translates the frozen architecture into small, dependency-ordered, verifiable construction stages. It does not authorize work outside V0.0.01 and does not replace the architecture's normative semantics. If implementation evidence invalidates a foundational decision, stop and use the architecture change standard before changing a durable contract.

## 1. Current repository audit

### 1.1 Audit scope and authority

The read-only audit covered the repository root at `/Users/abhisht/Documents/craxii`, including hidden entries, all files under `docs/`, repository guidance, implementation/configuration filename searches, file metadata, symlinks, and Git status discovery. The supplied workspace initially opened at `/Users/abhisht/Documents/craxii/docs`; the logical repository root is its parent directory even though that parent is not currently a Git worktree.

The authority order used for this plan is:

1. `docs/craxii-v0.0.01-architecture.md` and the user's planning instructions.
2. Any explicit later owner-approved amendment. None was found.
3. Long-term seams in `docs/craxii-identity-credential-architecture.md` where V0 does not explicitly defer or supersede them.
4. The superseded deep draft and prior architecture review as decision history only.
5. The annotated HTML as a teaching/rendering companion only.

The repository-level `AGENTS.md` still describes the identity/credential document as the primary design because it predates the frozen V0 architecture. For V0.0.01, that sentence is stale: the newer architecture and the present task explicitly establish `docs/craxii-v0.0.01-architecture.md` as authoritative. This is resolved by source-of-truth precedence and does not block implementation.

### 1.2 Exact repository contents

| Path | Audit finding | Implementation significance |
| --- | --- | --- |
| `AGENTS.md` | Repository guidance; states the workspace is documentation-only and records the Homebrew Node/npm PATH requirement. | Governs future local commands. It confirms there is no existing build or test system. |
| `docs/craxii-v0.0.01-architecture.md` | 4,876 lines; revision 1; last updated 2026-08-24; explicitly normative and superseding. | Primary source for every V0 domain, persistence, protocol, workstation, provider, client, deployment, recovery, and acceptance contract. |
| `docs/craxii-v0.0.01-architecture-annotated.html` | Generated 2026-08-24 from SHA-256 `da5d4ce9...`; that hash matches the current architecture Markdown. It adds explanations, a flowchart, and a worked story and states that they are non-normative. | Useful explanatory material, but not a later amendment and not an implementation source when wording differs. |
| `docs/temp/craxii-v0.0.01-architecture-review.md` | Prior review that identified work-item identity, causal context, journal, security, cancellation, replay, and crash-test corrections. | Decision history. Most amendments are incorporated in the frozen architecture. Its separate-executor recommendation is expressly superseded by the frozen one-user LocalWorkstation decision. |
| `CRAXII_V0.0.01_DEEP_ARCHITECTURE_SOURCE_OF_TRUTH.md` | Older 2,055-line deep draft. | Superseded for V0. It must not drive old turn-centric schemas, WebSocket command submission, or optional cancellation. |
| `docs/craxii-identity-credential-architecture.md` | Long-term identity, workload authority, provider credential, and project-isolation architecture. | Preserves the future external identity/Authority Service/RemoteWorkstation direction. Its mature services and multi-project VMs are explicit V0 deferrals. |
| `docs/craxii2.md` | Zero-byte placeholder with no documented purpose. | Must not be treated as a prototype, requirement, or target for reuse. |
| `.firecrawl/` | Empty directory. | No implementation or data source. |
| `.DS_Store` | macOS filesystem metadata. | Unrelated; should be ignored by future source control. |

There are no symlinks and no other source files in the audited tree.

### 1.3 Existing implementation inventory

The audit found none of the following:

- Rust source, `Cargo.toml`, `Cargo.lock`, Rust toolchain pin, backend crate, or build script;
- Swift source, Swift package, Xcode project/workspace, app target, unit-test target, or UI-test target;
- SQL migrations, SQLite database, fixtures, seed data, or persistence code;
- AWS Infrastructure as Code, EC2 provisioning, cloud-init, AMI manifest, KMS policy, security-group definition, or snapshot policy;
- systemd unit, sudoers policy, Caddyfile, TLS configuration, deployment script, release manifest, or rollback script;
- unit, integration, protocol, provider, workstation, client, end-to-end, crash-injection, or restore tests;
- CI workflow, formatter/linter configuration, dependency policy, release automation, or developer task runner;
- prototypes or abandoned executable code whose behavior needs preservation.

The directory is not a Git repository and no parent `.git` directory is present. Consequently, there is no history, branch, remote, tag, commit convention, or existing Git revision to embed in a binary. This is repository setup work, not a Craxii architecture blocker.

### 1.4 True implementation starting point

Implementation starts from an empty application repository with a detailed architecture. No application behavior can be marked complete or reused. Every future green test must originate from newly created code, migrations, configuration, or operational assets.

The first implementation commit must therefore establish source control and a minimal Rust workspace before any domain or adapter work. The macOS app must be created later, after protocol contracts are executable and tested headlessly. AWS deployment must be created only after the backend is deterministic on Ubuntu.

### 1.5 Frozen decisions already incorporated from review

The frozen architecture has already resolved the important older ambiguities:

- `work_item`, not a conversation turn, is the durable execution and recovery unit;
- immutable `craxii_id`, workstation/workspace identity, runtime identity, causal inputs, and stable execution IDs exist;
- queued later messages are excluded by work ordinal/input relationship rather than latest-row queries;
- the journal has global cursor, stream sequence, causation, correlation, payload version, and projection consistency;
- model selection precedes model-specific context rendering;
- provider output is an ordered collection of items;
- HTTP owns durable commands; WebSocket owns replay/live delivery;
- cancellation is required;
- intent precedes provider/tool action and observed outcome follows it;
- ambiguous tool side effects become `outcome_unknown`, and owning work becomes `interrupted` without automatic repetition;
- V0 uses one non-root `craxii` Unix user with an explicit admin path, not the older separate executor user;
- Caddy terminates TLS and the benchmark deployment should use a separate encrypted data EBS volume.

Future implementation must not reopen these choices merely because an older document recommends something else.

### 1.6 Repository/architecture mismatches

| Frozen expectation | Repository reality | Required response |
| --- | --- | --- |
| One repository containing backend, native client, migrations, operations, and docs | Only docs exist. | Create the layout incrementally in Stages 1, 21, and 27; do not pretend any milestone is partially implemented. |
| Version/build metadata includes Git revision | No Git metadata exists. | Establish source control in Stage 1; permit an explicit `unversioned` local value only before the first repository commit, and forbid it in release artifacts. |
| SQLx migrations define versioned schema | No Cargo project or migrations exist. | Build migration harness before any repository method in Stages 5–8. |
| Native macOS client is the product surface | No Xcode project exists. | Create it only after protocol/replay behavior is frozen by executable tests. |
| Ubuntu 24.04/cgroup/systemd semantics are testable | Current workspace is on macOS and has no Linux harness. | Add a disposable Ubuntu test/deployment environment; never treat macOS process tests as proof of Linux cleanup. |
| Deployment is repeatable | No AWS, DNS, Caddy, systemd, or backup assets exist. | Build operations only after local correctness gates; keep environment-specific IDs out of domain types. |

### 1.7 Prototype and reuse verdict

There is no executable prototype to reuse or delete. `docs/craxii2.md` is an empty placeholder, not abandoned code. The old deep source and review contain superseded conceptual shapes and must not be copied mechanically. In particular, do not reuse turn-centric state, `conversation_id + sequence_no` as the whole journal envelope, WebSocket command submission, a binary text-or-tools response union, optional cancellation, or inherited shell environments.

### 1.8 Blocker audit

#### Architecture blockers

None remain. The architecture defines enough identity, state, transaction, failure, provider, workstation, protocol, client, security, deployment, and recovery behavior to plan and begin Stage 1.

The current official OpenAI Responses API continues to expose custom function tools, streaming, ordered output items, explicit `store`, parallel-tool-call control, usage, and stateless encrypted reasoning continuation support. Exact request fields must still be reverified immediately before adapter implementation because provider schemas evolve. See the [official Create a model response reference](https://developers.openai.com/api/reference/cli/resources/responses/methods/create).

#### Implementation and deployment prerequisites, not blockers

The following are intentionally supplied or chosen at the stage that first needs them:

- a Git repository and, before CI/release, a remote hosting location;
- a pinned stable Rust toolchain and an x86-64 Ubuntu 24.04 build/test path;
- current Xcode and a chosen minimum supported macOS version;
- an Apple development team/signing identity if distributing beyond local development;
- an OpenAI development project, revocable spend-limited API key, permitted model ID, rate limits, and current model capability/context-limit values;
- an AWS account, region, availability zone, VPC/subnet choice, x86-64 Ubuntu 24.04 AMI, instance type, KMS key policy, and billing approval;
- a DNS hostname, control of its records, Caddy ACME contact, and the current trusted client source CIDR;
- a first random 256-bit device token and device display name for out-of-band provisioning;
- backup retention approval, snapshot owner, restore-test instance budget, and an operator responsible for recording recovery-point/recovery-time evidence;
- an explicit absence of production/customer/catastrophic credentials on the VM.

None changes a durable architecture contract. Missing values block only the live stage that consumes them.

## 2. Implementation principles

### 2.1 Non-negotiable invariants

Every stage must preserve these rules:

1. Craxii identity is `craxii_id`; no model, provider response, client, PID, VM, volume, hostname, path, or conversation substitutes for it.
2. A new conversational user message and exactly one queued work item commit atomically with input linkage, journal events, and idempotent command response.
3. SQLite plus committed evidence artifacts is canonical V0 control state; RAM, tasks, sockets, provider state, client caches, and traces are not.
4. The journal is append-only, sequence-based, versioned, causally linked, and transactionally consistent with current-state projections.
5. No database transaction remains open across provider I/O, workstation I/O, artifact streaming, WebSocket delivery, or another external wait.
6. Provider and workstation intent is durable before action. A claimed observed outcome is durable only after observation and evidence finalization.
7. Ambiguous side effects are never retried automatically and are never recast as definite success or failure.
8. One conversation has at most one active work item. A later accepted message remains queued and cannot enter earlier work context.
9. Model selection precedes final context rendering. Full causally eligible history either fits or fails explicitly; V0 does not compact, summarize, retrieve, or silently truncate it.
10. The explicit Rust agent loop decides continuation and terminality. No provider, SDK, framework, tool handler, callback, or client owns iteration.
11. Provider wire types remain inside their adapter. SQLx types remain inside SQLite. OS process/filesystem types remain inside LocalWorkstation. Public protocol types are Craxii-owned.
12. HTTP is the durable command substrate. The journal cursor is durable delivery truth. WebSocket notification and drafts are replaceable, lossy wakeup/presentation mechanisms around that truth.
13. Backend execution is non-root; elevation is explicit, recorded, noninteractive, and performed through LocalWorkstation with a clean child environment.
14. Traces explain process behavior but never reconstruct product history. Content, commands, outputs, headers, tokens, and secrets are redacted by default.
15. A stage is complete only when its test gate passes on the environment whose semantics it claims.

### 2.2 Boundary and migration-cost matrix

| Subsystem | Architecture-critical now | Implementation detail allowed to vary | Deliberately deferred | High-cost mistake to prevent |
| --- | --- | --- | --- | --- |
| Repository | One coherent backend with domain/application/ports/adapters dependency direction | Exact internal module filenames | Multiple services/workspaces/crates without need | Framework or adapter types leaking into domain contracts |
| Identity/domain | Typed opaque UUIDv7 IDs; logical workstation/workspace identities | UUID library and display helpers | Cryptographic Craxii key/control-plane identity | VM, path, conversation, provider, or PID becoming identity |
| State machines | Legal transitions, certainty, ownership, terminal immutability | Enum/module layout | In-flight resume and terminal-work continuation | Treating missing outcome as failure or making terminal state reversible |
| Persistence | SQLite WAL/FULL, journal/projection atomicity, intent-specific transactions | SQL organization and repository struct split | PostgreSQL/distributed state | SQLx types in application; generic repository; event/projection drift |
| Journal | Global cursor, per-stream sequence, versioned typed payloads, causation/correlation | Serialization helper organization | WORM/hash chain/pruning | Timestamp ordering; mutable history; cursor tied to WebSocket |
| Artifacts | Stable ID/hash/provenance, rename-before-reference, bounded evidence | Dedup implementation/compression | S3 and automatic retention | Committed DB reference to absent bytes; local path as public identity |
| Commands/scheduler | Atomic one-message-one-work; scoped idempotency; durable FIFO; one active work | Wakeup primitive and polling cadence | Priority, steering, parallel work, background triggers | In-memory queue as truth; retry creating duplicate work |
| Context | Exact causal eligibility/order/manifest; explicit limit failure | Canonical rendering structs and conservative estimator | Memory, compaction, retrieval | `all messages so far` query leaking later queued input |
| Models | Craxii-owned ordered types, deterministic target selection, per-attempt evidence | Target config format and adapter-native typed options | Second real provider and sophisticated routing | OpenAI type/history becoming canonical; final-text/tool union |
| Agent loop | Explicit bounded loop; persisted attempts; sequential tools | Helper function decomposition | Framework orchestration, subagents, parallel tools | Hidden recursive model calls or side-effect retries |
| Tools | Immutable registry, schema/decoder equivalence, authority seam, service-owned journal order | Schema-generation crate and handler layout | MCP/browser/cloud/database tools | Handler writing journal or unknown tool falling back to shell |
| Workstation | Stable execution ID, logical paths, explicit cwd/env/privilege, inspect/cancel/cleanup | Linux process/cgroup library | RemoteWorkstation implementation | Direct `Command` outside adapter; PID/path as durable contract |
| Protocol/events | Version 1 envelopes, HTTP commands, bootstrap high-water, cursor replay/live handoff | JSON field helpers and connection implementation | Cursor epochs/history pruning | WebSocket as ack/state; provider events in client protocol |
| macOS client | Thin disposable projection, Keychain token, stable IDs, replay convergence | Swift package organization and AppKit usage | Other platforms/account UX | Client cache/draft becoming canonical or new ID on retry |
| Security | Dev-only trust domain, no production authority, secret isolation from context/children/logs | Token provisioning CLI and file modes | Mature Authority Service/project isolation | Claiming same-host root containment or exposing provider key to tools |
| Observability | Required IDs, timings, counts, usage, cleanup, replay, recovery; redaction | JSON formatter and query/report tooling | Prometheus/collector/eval platform | Tracing as journal or high-cardinality IDs as metric labels |
| Deployment/recovery | Ubuntu x86-64, encrypted EBS, systemd, Caddy, snapshots, restore rehearsal | Terraform versus equivalent declarative provisioning | HA/zero downtime/automatic replacement | Incompatible rollback, raw DB copy under WAL, guest-controlled backup deletion |

### 2.3 Build-small and verification policy

- Each substage introduces one coherent responsibility and its tests.
- Domain behavior is proven before adapters; local fakes are proven before real external calls; backend protocol is proven before native UI; Ubuntu semantics are proven before EC2 acceptance.
- Test code may expose deterministic clocks, scripted providers, fake workstations, and failpoints. Production composition must not expose test-only behavior.
- Every durable mutation test inspects both the detailed/current row and journal events. Every external-side-effect test checks intent order and outcome certainty.
- Every client test distinguishes optimistic state, ephemeral draft state, and committed durable state.
- Every final benchmark retains a machine-readable evidence bundle: configuration fingerprint, binary/schema/protocol versions, IDs/cursors, database queries, trace excerpts, artifact hashes, and pass/fail assertions, all redacted.
- No stage may broaden V0 by adding memory, provider-owned continuity, higher-level workstation methods, more tools/providers, background scheduling, production authority, distributed infrastructure, or a web client.

### 2.4 Definition of “coherent at every gate”

A stage may temporarily leave later functionality unavailable, but the current binary must fail closed and explain why. Examples: before migrations exist it may expose only liveness; before a model target is configured readiness remains false; before the scheduler/agent loop is wired, production message acceptance must not claim that work will execute. Test-only adapters may drive incomplete layers only through explicit test composition.

No placeholder should silently return success. Unsupported routes, tools, provider items, state versions, schema versions, or execution modes return typed errors.

## Implementation Planning Challenge

### Atomic model-attempt transaction versus failpoint labels

**Architecture assumption:** The context manifest, model invocation intent, work transition to `waiting_on_model`, and corresponding journal events commit in one model-attempt transaction. The final assistant message and `work.completed` transition likewise commit in one final-answer transaction.

**Repository reality/problem:** No failpoint implementation exists, and the required failpoint list uses labels such as `after_context_manifest_commit`, `after_model_intent_commit`, and `after_assistant_message_commit`. Taken literally, the first two imply separate context and intent commits, while the transaction section requires them to be atomic; the assistant label could similarly be misread as a partial final-answer commit.

**Consequence:** Implementing literal intermediate commits would create recovery states the architecture forbids and increase migration cost. Treating the labels as identical would leave important rollback and post-commit delivery windows untested.

**Proposed resolution:** Preserve the normative transaction boundaries. Define precise hooks as follows:

- an intra-transaction hook after manifest rows are written but before model-intent rows/commit, whose forced process death must roll the whole transaction back;
- an intra-transaction hook after all model-attempt rows/events are written but before commit, also expected to roll back;
- a post-commit hook after the complete model-attempt transaction and before provider I/O;
- an intra-transaction final-answer hook before commit and a post-commit hook before client notification.

Keep compatibility aliases matching the architecture's named failpoints in the test controller, but document the exact physical boundary and expected durable state for each. Never split atomic domain transactions merely to create a crash point.

**Blocking status:** Non-blocking. Stage 2 fixes the test vocabulary before persistence work, and Stages 17–18 prove the resulting semantics.

## Stage 1: Repository foundation and engineering governance

### Objective

Turn the documentation-only directory into a versioned, minimal, reviewable implementation repository with one Rust backend package and explicit quality/dependency rules.

### Why it happens now

Every later stage needs stable paths, a lockfile, reproducible tool versions, test commands, and a dependency direction. Creating domain or migration files first would force those decisions implicitly.

### Preconditions

- The audit and source-of-truth precedence above are accepted.
- No existing executable files need preservation.
- A stable Rust toolchain can be installed or selected locally.

### Exact implementation work

- Execute Substages 1.1–1.3 in order.
- Establish only the backend and repository assets needed now; create macOS and operations projects in their later stages.
- Record the baseline verification command and dependency-approval process in repository guidance.

### Data/state introduced

Git metadata, root workspace manifests, a Cargo lockfile, toolchain pin, ignore rules, dependency-decision records, and CI/local verification definitions.

### Contracts/interfaces introduced

The repository boundary, Rust module dependency direction, supported build targets, and the command developers use to decide whether a change is mergeable.

### Failure behavior

Toolchain or lint failures fail the stage without being treated as product failures. A missing Homebrew Node/npm executable is retried after exporting `/opt/homebrew/bin:/opt/homebrew/sbin` into `PATH`, per `AGENTS.md`.

### Validation

Run the empty/minimal backend's format, lint, build, and test commands; inspect the dependency tree; verify a clean source-control status after committing the baseline; verify no generated secrets/build output is tracked.

### Exit criteria

- [ ] The repository is under source control and has an unambiguous root.
- [ ] One minimal Rust backend package builds through the pinned toolchain.
- [ ] `Cargo.lock` is committed and quality commands are deterministic.
- [ ] Module ownership and dependency governance are documented.
- [ ] No application behavior has been implied or stubbed as successful.

### What is deliberately NOT implemented yet

Configuration semantics, database access, HTTP routes, provider code, tools, the agent loop, the native app, deployment, and product state.

### Substages

#### Substage 1.1: Establish source control and root layout

##### Objective

Create the durable repository baseline from the audited documentation-only tree.

##### Why it happens now

Build metadata, migrations, release checksums, and auditability all require a real revision history before implementation accumulates.

##### Preconditions

The audited files remain unchanged except for this implementation plan.

##### Exact implementation work

- Initialize Git at `/Users/abhisht/Documents/craxii` if the project is not being imported into an existing upstream repository.
- Add ignore rules for `.DS_Store`, Rust `target/`, Xcode `DerivedData/`, Swift build state, local databases/WAL/SHM, artifacts, secret/config overrides, Terraform state, and test evidence containing sensitive data.
- Reserve top-level ownership: `backend/` for Rust, `clients/macos/` for the native app, `ops/` for deployment/operations, `scripts/` for repository checks, and `docs/` for architecture/plan/decision records. Create directories only when their first real file is added.
- Preserve existing docs; mark the empty `docs/craxii2.md` as unused in review rather than inventing content for it.
- Choose a remote host before CI is enabled; do not block local work on that choice.

##### Data/state introduced

Git object history, ignore policy, and canonical top-level ownership.

##### Contracts/interfaces introduced

Repository paths become ownership boundaries; external project checkouts never live inside this source repository.

##### Failure behavior

If this directory must be imported into another repository, stop before creating nested `.git` metadata and perform the import there. Do not manufacture a revision string.

##### Validation

`git status` lists only intentional files; a search confirms no credential, database, build output, Terraform state, or generated artifact is tracked.

##### Exit criteria

- [ ] One repository root is established.
- [ ] Ignore rules cover all known local/generated/secret state.
- [ ] Existing architecture documents retain their hashes.

##### What is deliberately NOT implemented yet

Branch protections, release tags, CI/CD deployment, monorepo tooling, or multiple product repositories.

#### Substage 1.2: Create the minimal Rust workspace and backend package

##### Objective

Create a compilable `craxii-server` package whose structure enforces the frozen ownership boundaries without premature crate fragmentation.

##### Why it happens now

Domain, persistence, and adapters require a compiler-visible home and a library target that integration tests can compose.

##### Preconditions

Substage 1.1 is complete and a supported stable Rust toolchain is available.

##### Exact implementation work

- Add root `Cargo.toml`, committed `Cargo.lock`, `rust-toolchain.toml`, and narrowly scoped Cargo configuration.
- Add one package under `backend/` with a library target for domain/application/ports/adapters and a thin `craxii-server` binary composition root.
- Establish modules matching `bootstrap`, `domain`, `application`, `ports`, and `adapters`; do not add empty trait layers that have no caller.
- Set Rust edition, minimum supported Rust policy, release profile, panic/backtrace policy, and target metadata deliberately.
- Add the smallest startup path that exits successfully only after constructing an empty application shell; it must not report Craxii readiness.

##### Data/state introduced

Cargo workspace/package metadata, compiler target policy, and the initial binary/library artifacts.

##### Contracts/interfaces introduced

`main` is composition-only; domain imports no adapter/framework crate; adapters point inward through ports/application/domain.

##### Failure behavior

Unsupported compiler versions and target architectures fail at build/startup with an explicit message. No fallback downloads or runtime code generation occur.

##### Validation

Run `cargo check --workspace --all-targets`, build debug and release binaries, and use dependency inspection to confirm the minimal graph.

##### Exit criteria

- [ ] The package builds on macOS for developer feedback.
- [ ] The package has a documented x86-64 Linux build path.
- [ ] Library and binary roles are distinct.
- [ ] No Axum, SQLx, Reqwest, or provider behavior is hidden in `main`.

##### What is deliberately NOT implemented yet

Multiple backend crates, plugin systems, dynamic loading, database migrations, or an HTTP listener.

#### Substage 1.3: Establish quality, dependency, and development workflow gates

##### Objective

Make every later stage use the same reproducible formatting, linting, testing, dependency-review, and documentation checks.

##### Why it happens now

Adding policies after dependencies and unsafe process code arrive would normalize avoidable boundary violations.

##### Preconditions

Substage 1.2 builds successfully.

##### Exact implementation work

- Add a single local verification entry point that runs Rust formatting, Clippy with warnings denied, unit/integration tests, documentation checks, and architecture-boundary checks as they become available.
- Record approved dependency categories and require a short decision record for parsing, persistence, secrets, unsafe/Unix, schema generation, and cloud tooling dependencies.
- Add license/advisory review appropriate to the chosen ecosystem tools; pin direct dependencies and commit lockfiles.
- Define CI jobs for macOS compile/unit/client tests and Ubuntu 24.04 backend/integration tests once a remote exists. Keep privileged cgroup/sudo tests in an explicit target-host job rather than weakening them to fit a generic runner.
- Make Markdown lint use the repository's Node PATH rule. Avoid introducing an npm project solely for one lint command unless repeatability requires it.

##### Data/state introduced

Verification scripts/workflows, dependency records, tool configuration, and test classification metadata.

##### Contracts/interfaces introduced

The merge gate distinguishes portable unit tests, file-backed SQLite tests, Ubuntu workstation tests, live-provider smoke tests, and destructive/disposable crash tests.

##### Failure behavior

Any mandatory check fails the build. Live external tests are opt-in and must report “not configured” distinctly from pass; release gates require them explicitly.

##### Validation

Run the verification entry point locally, deliberately introduce a formatting/lint failure to prove it is detected, and inspect that secrets and test evidence are excluded.

##### Exit criteria

- [ ] One documented verification command exists.
- [ ] Required checks cannot silently skip.
- [ ] Dependency additions have an owner and review rule.
- [ ] Platform-specific tests are classified honestly.

##### What is deliberately NOT implemented yet

Automated production deployment, broad supply-chain platforms, code-coverage targets divorced from risk, or CI substitutes for the EC2 acceptance environment.

## Stage 2: Typed bootstrap, metadata, telemetry, and failure-test seams

### Objective

Create the process shell that can parse safe configuration, identify its build/runtime, emit redacted diagnostics, expose honest health state, and support deterministic crash injection in tests.

### Why it happens now

Persistence and external adapters need configuration, clocks, IDs, tracing, and failpoints. These cross-cutting seams must exist before behavior is scattered through subsystems.

### Preconditions

Stage 1 is complete and its verification command passes.

### Exact implementation work

- Execute Substages 2.1–2.3.
- Keep readiness false until later stages register and validate storage, recovery, tools, and the default model target.
- Make release builds incapable of activating test failpoints.

### Data/state introduced

Typed config snapshots, architecture/protocol/schema/build version constants, redacted secret wrappers, process metadata, trace events, readiness state, and test-only failpoint markers.

### Contracts/interfaces introduced

Configuration ownership, secret lookup, `Clock`, build metadata, health-state transitions, telemetry redaction, and failpoint-controller contracts.

### Failure behavior

Unknown/malformed config, missing required secret references, invalid bounds, or inconsistent versions fail before product-state mutation. Telemetry initialization failure is fatal when it would leave the service unauditable.

### Validation

Use table-driven configuration tests, secret-redaction tests, deterministic clocks, build-metadata assertions, health-state tests, and a subprocess killed at a harmless failpoint.

### Exit criteria

- [ ] Startup failures are typed and redacted.
- [ ] Local and EC2 configuration shapes are separated from secrets.
- [ ] Every process can report exact build/protocol/architecture metadata.
- [ ] Failpoints are deterministic in tests and absent from release behavior.

### What is deliberately NOT implemented yet

SQLite startup, provider credentials, client device tokens, AWS secret retrieval, domain persistence, or a ready product service.

### Substages

#### Substage 2.1: Define typed configuration and compatibility metadata

##### Objective

Create a strict, versioned configuration model that covers future V0 settings without embedding environment-specific values in domain code.

##### Why it happens now

Every adapter and operational stage consumes configuration; a late generic environment-variable layer would leak strings and defaults everywhere.

##### Preconditions

The minimal backend package exists.

##### Exact implementation work

- Define typed sections for server bind/public URL, state/artifact/workspace paths, SQLite tuning, workstation identity/generation, model targets/default, provider timeouts/retries, agent/tool/output limits, shell/environment policy, device-auth source, tracing, shutdown, and failpoint mode.
- Parse a non-secret TOML file with unknown keys rejected and semantic validation across fields.
- Represent secrets only by logical credential reference; support a local development secret source and a later systemd-credential source behind one bootstrap-only interface.
- Define explicit architecture, protocol, configuration, and maximum-supported schema versions.
- Validate bounds against architecture hard limits rather than accepting arbitrary operator values.

##### Data/state introduced

Immutable per-process `ValidatedConfig`, version constants, and a non-secret configuration fingerprint safe for traces/deployment evidence.

##### Contracts/interfaces introduced

Only bootstrap/adapters see physical paths and credential sources; application/domain receive typed operational values and logical identities.

##### Failure behavior

Unknown key, unsafe public bind, missing default target, invalid URL/path, limit inversion, or unsupported version is a definite startup error. Raw parsed config is never used after validation.

##### Validation

Golden valid configs for local test and EC2; negative fixtures for every invalid bound/key/reference; serialization/fingerprint stability tests.

##### Exit criteria

- [ ] All V0 configuration categories have typed owners.
- [ ] Defaults are explicit and architecture-compliant.
- [ ] Secret material cannot appear in the non-secret fingerprint.

##### What is deliberately NOT implemented yet

Dynamic reload, remote configuration service, feature flags that alter semantics, per-user settings, or provider key loading.

#### Substage 2.2: Establish build/runtime metadata, clocks, and redacted tracing

##### Objective

Make process behavior measurable and time semantics testable before business logic is added.

##### Why it happens now

All later records require UTC timestamps, monotonic durations, correlation IDs, build revision, and safe diagnostics.

##### Preconditions

Substage 2.1 defines version/config inputs.

##### Exact implementation work

- Embed package version, Git revision, build target, and build timestamp policy; reject `unversioned` for release deployment.
- Define a `Clock` port returning wall-clock UTC and monotonic instants; implement system and deterministic test clocks.
- Initialize pretty local tracing and JSON service tracing with a shared redaction policy and stable subsystem/span names.
- Create secret/token wrapper types whose debug/display/serialization paths redact by construction.
- Define health state as live/unready/ready/draining/fatal with reason codes safe for restricted diagnostics.

##### Data/state introduced

Build metadata snapshot, process start timestamp, monotonic timing handles, trace correlation fields, and in-memory health status.

##### Contracts/interfaces introduced

All durations use monotonic clocks; durable timestamps use UTC; content/secrets are excluded unless an explicit bounded diagnostic artifact policy allows them.

##### Failure behavior

Clock conversion or tracing sink failures surface as normalized startup/internal errors. Logging falls back only to a safe local sink, never to unredacted output.

##### Validation

Snapshot JSON trace fields, scan output for sentinel secrets/authorization headers, verify deterministic duration tests, and ensure health never reports ready by default.

##### Exit criteria

- [ ] Version and revision appear in startup diagnostics.
- [ ] Secret wrappers cannot accidentally print raw values.
- [ ] Tests can control wall and monotonic time independently.
- [ ] Health state is honest and thread-safe.

##### What is deliberately NOT implemented yet

Prometheus, OpenTelemetry collectors, production log shipping, product journal events, or content logging.

#### Substage 2.3: Build the test-only failpoint and subprocess controller foundation

##### Objective

Provide deterministic named crash windows before any durable semantics need to be tested.

##### Why it happens now

Retrofitting failpoints after transaction and process code exists risks hooks at the wrong side of a commit or side effect.

##### Preconditions

Build features, tracing, and process metadata exist.

##### Exact implementation work

- Add a test-only failpoint registry keyed by the architecture names plus precise physical-boundary metadata: before commit, after commit, before I/O, after observed I/O, or during cleanup.
- Gate it behind a non-default test feature and a cryptographically irrelevant local test control channel/file descriptor unavailable in release composition.
- Build an integration controller that launches the backend as a subprocess, waits for a structured marker, sends `SIGKILL`/`SIGTERM`, reopens state, and captures redacted logs.
- Define aliases/resolution for the atomic-transaction challenge above.
- Reserve the complete required failpoint list; activate each hook only in the stage that owns the boundary.

##### Data/state introduced

Ephemeral failpoint configuration, marker events, subprocess exit evidence, and a crash-test result manifest. None is canonical product state.

##### Contracts/interfaces introduced

A failpoint documents its exact side of transaction/action and expected durable result. Production code calls a zero-cost/disabled hook only at reviewed boundaries.

##### Failure behavior

Unknown or release-mode failpoint configuration fails closed. A test times out if the marker is not reached and must not infer a crash window from elapsed time alone.

##### Validation

Kill a harmless test process before and after a dummy durable-file rename, verify controller classification, and inspect the release binary/config surface for no activatable failpoint.

##### Exit criteria

- [ ] The controller can deterministically kill at a named marker.
- [ ] Pre/post boundary expectations are machine-readable.
- [ ] Release behavior cannot enable failpoints.
- [ ] No domain transaction has been split for test convenience.

##### What is deliberately NOT implemented yet

The actual message/model/tool/artifact/final-answer hooks, systemd integration, or randomized chaos testing.

## Stage 3: Canonical domain foundations

### Objective

Define provider-, database-, transport-, and operating-system-independent identities, entities, content, time/evidence references, and errors.

### Why it happens now

State machines, schema, protocols, and adapters all need one canonical vocabulary before persistence types or external wire formats exist.

### Preconditions

Stages 1–2 pass and the architecture's identifier/time/error policies are accepted.

### Exact implementation work

- Execute Substages 3.1–3.3.
- Keep domain construction guarded; adapters may parse strings but cannot bypass invariants.
- Add compile-time/import-boundary tests where practical.

### Data/state introduced

Typed IDs, ordered sequence/ordinal values, immutable content blocks/hashes, entity snapshots, execution references, and normalized error values.

### Contracts/interfaces introduced

Canonical Rust domain types and validation constructors used by every later port and protocol mapper.

### Failure behavior

Invalid UUID/text/hash/enum/limit inputs return typed validation errors. Domain types never panic on untrusted input and never carry raw external error bodies.

### Validation

Property/table tests for ID round trips, cross-ID type safety, content canonicalization/hash stability, time formatting, entity validation, and error redaction.

### Exit criteria

- [ ] Every architecture-required durable ID has a distinct type.
- [ ] Canonical ordering is sequence/ordinal based, never timestamp based.
- [ ] Content and errors are stable and provider/database independent.
- [ ] No adapter/framework dependency enters `domain`.

### What is deliberately NOT implemented yet

Lifecycle transition services, persistence rows, public JSON protocol, provider requests, machine I/O, or cryptographic Craxii identity.

### Substages

#### Substage 3.1: Implement typed IDs, ordering, time, and hashes

##### Objective

Prevent identity substitution and timestamp-based ordering at the type level.

##### Why it happens now

Schema keys, journal causation, model/tool pairing, replay cursors, and client IDs depend on these primitives.

##### Preconditions

The domain module exists and UUID/hash dependencies have approved records.

##### Exact implementation work

- Add UUIDv7 newtypes for every required durable/public identity: Craxii, conversation, message, work, workstation, workspace, runtime, event, invocation/logical invocation, context manifest, tool execution, artifact, device, client command/message, execution, and draft.
- Parse/serialize lowercase canonical strings; reject noncanonical or wrong-version inputs at boundaries where required.
- Add positive integer wrappers for journal offset, stream sequence, work ordinal, agent step, tool ordinal, and attempt number with checked increments.
- Add UTC RFC 3339 microsecond timestamp value and monotonic-duration types; forbid duration reconstruction from persisted wall time.
- Add SHA-256 digest and canonical byte-count types with bounded conversions.

##### Data/state introduced

Pure values only; no persistence.

##### Contracts/interfaces introduced

Distinct ID types cannot be interchanged; order wrappers define comparison and JSON/SQLite-safe ranges.

##### Failure behavior

Overflow, zero sequence, malformed ID/digest, or invalid timestamp is definite `domain_validation` failure.

##### Validation

Round-trip/property tests, compile-fail tests or typed API tests for ID misuse, UUIDv7 generation uniqueness, checked overflow tests, and Swift `Int64` cursor compatibility fixtures.

##### Exit criteria

- [ ] All required IDs and ordinals exist.
- [ ] No ordering helper uses UUID time or wall clock.
- [ ] Stable string/JSON forms are tested.

##### What is deliberately NOT implemented yet

Database allocation, Snowflake IDs, provider IDs as domain IDs, or distributed ordering.

#### Substage 3.2: Define principal, conversation, message, work, workstation, workspace, and evidence references

##### Objective

Represent the durable concepts and their relationships without persistence concerns.

##### Why it happens now

The state machines and migrations require precise fields and constructors, especially one-message-one-work and logical workspace identity.

##### Preconditions

Substage 3.1 types exist.

##### Exact implementation work

- Define `CraxiiPrincipal`, primary `Conversation`, immutable `Message`, versioned text `ContentBlock`, `WorkItem`, and `WorkItemInput` with the architecture fields and V0 enum constraints.
- Define `WorkstationIdentity`, generation, capabilities snapshot, `WorkspaceIdentity`, logical root/path reference, and physical resolved-path evidence separated from identity.
- Define artifact metadata/reference, provider/model target reference, model/tool attempt reference, authority-decision snapshot, and runtime-instance metadata without adapter-specific payloads.
- Enforce one primary conversation/kind and V0 one `trigger` input in application constructors while retaining relation-shaped types for future inputs.
- Define canonical content hashing from length-prefixed normalized blocks so idempotency does not depend on JSON key order.

##### Data/state introduced

In-memory immutable aggregate/value representations and canonical hashes.

##### Contracts/interfaces introduced

Messages are immutable; work owns execution; workspace ID/logical path is canonical while resolved absolute path is evidence; physical hosting fields do not define Craxii.

##### Failure behavior

Missing paired client fields, invalid role/content, unsupported work kind/relationship, inconsistent workstation generation, or invalid logical path fails construction.

##### Validation

Constructor tests for every invariant, content hash fixtures shared later with Swift, and negative tests showing absolute paths/EC2 IDs cannot substitute for logical IDs.

##### Exit criteria

- [ ] One-message-one-work can be expressed without storing a message ID directly as the sole work input.
- [ ] Future work triggers remain schema-capable but protocol-inaccessible.
- [ ] Physical and logical workstation concepts are separate.

##### What is deliberately NOT implemented yet

Multiple visible conversations, steering, schedules, background work, remote workstations, images/files, or memory.

#### Substage 3.3: Define normalized errors, certainty, retryability, and safe detail

##### Objective

Create one stable error vocabulary before SQLx, provider, OS, or HTTP errors appear.

##### Why it happens now

Every boundary must translate failures consistently, and retryability must not accidentally authorize tool repetition.

##### Preconditions

Core IDs/value types exist for correlation.

##### Exact implementation work

- Define architecture categories, stable codes, `never|bounded|user_action|operator_action`, and `definite|outcome_unknown` certainty.
- Separate safe user/client message, structured source status, and internal trace-only detail.
- Add constructors/mappers for validation, state conflict, storage, context, selection, provider, tool, authority, workstation, artifact, cancellation, protocol, authentication, and invariant failures.
- Make error serialization omit causes/backtraces/secrets by default.
- Document that retryability is advice to the owning policy and never implies automatic side-effect retry.

##### Data/state introduced

Normalized error values suitable for detailed rows, journal payload references, protocol projection, and tracing.

##### Contracts/interfaces introduced

Adapters translate external failures once; application decisions use category/code/certainty rather than library error types.

##### Failure behavior

Unknown adapter errors map to a conservative internal/provider/workstation code with definite versus unknown chosen from the action boundary, not guessed from text.

##### Validation

Golden serialization tests; sentinel secret/backtrace redaction; exhaustive category mapping; tests that `bounded` provider retry and `outcome_unknown` tool results lead to different policies.

##### Exit criteria

- [ ] All architecture categories are represented.
- [ ] Safe and internal detail cannot be confused by API types.
- [ ] Certainty is mandatory where side effects may exist.

##### What is deliberately NOT implemented yet

Localized messages, automatic remediation, generic error-string matching, or client display decisions.

## Stage 4: Lifecycle state machines and terminal decision rules

### Objective

Make work, model invocation, tool execution, cancellation, and terminal-output legality executable and exhaustively tested before persistence mutates them.

### Why it happens now

Migrations and transaction methods must encode known transitions rather than discovering lifecycle semantics in SQL or handlers.

### Preconditions

Stage 3 domain primitives and normalized errors are complete.

### Exact implementation work

- Execute Substages 4.1–4.3.
- Express transitions as pure decisions returning next state plus required semantic effects, not performing I/O.
- Generate a reviewed transition matrix used by unit and later repository tests.

### Data/state introduced

Work/model/tool lifecycle values, state versions, transition commands/results, terminal reasons, cancellation decisions, and recovery classifications.

### Contracts/interfaces introduced

Pure state-machine APIs become the only application authority for legal lifecycle changes.

### Failure behavior

Illegal/stale transitions return `state_conflict` or `internal_invariant_error`; they never coerce, skip, or overwrite terminal state.

### Validation

Exhaustive legal/illegal transition tests, property tests for terminal immutability and version increments, and race-decision tables for cancellation versus late outcomes.

### Exit criteria

- [ ] Every architecture state/transition has a test.
- [ ] `outcome_unknown` is only a tool-attempt terminal state; work becomes `interrupted`.
- [ ] Cancellation completion requires confirmed cleanup.
- [ ] Runtime terminality does not depend solely on provider stop reason.

### What is deliberately NOT implemented yet

Database updates, task scheduling, provider invocation, process signals, or in-flight continuation after restart.

### Substages

#### Substage 4.1: Implement the work-item state machine

##### Objective

Encode the exact queued/active/terminal lifecycle and guards.

##### Why it happens now

The work projection, claim transaction, cancellation service, recovery, and agent loop all depend on one definition.

##### Preconditions

`WorkItem`, runtime ownership, attempts, versions, and terminal reasons exist.

##### Exact implementation work

- Implement all legal transitions among `queued`, `running`, `waiting_on_model`, `waiting_on_tool`, `cancel_requested`, `completed`, `failed`, `cancelled`, and `interrupted`.
- Require expected state/version, runtime ownership where active, correct current attempt for waiting states, and assistant message for completion.
- Clear current attempts/runtime ownership on every terminal transition.
- Encode queued cancellation and active cancellation separately.
- Define active-state set once for both domain tests and migration/index generation review.

##### Data/state introduced

Pure transition decisions with next version, timestamps required, terminal reason, current-attempt changes, and required event kind.

##### Contracts/interfaces introduced

Terminal work is immutable; claim requires FIFO/one-active conditions supplied by repository; completion requires one atomic assistant-message effect.

##### Failure behavior

Stale owner/version or illegal transition returns a conflict; corrupted combinations return invariant failure.

##### Validation

Full state-pair matrix, terminal-state property tests, state-version overflow tests, and cancellation/outcome race cases.

##### Exit criteria

- [ ] No transition exists outside the architecture graph.
- [ ] Required associated records are explicit inputs.
- [ ] Active-state definition is reusable by persistence tests.

##### What is deliberately NOT implemented yet

Resume of terminal work, priority, steering, parallel active work, or automatic retry work items.

#### Substage 4.2: Implement model- and tool-attempt state machines

##### Objective

Encode durable intent, observation, interruption, and certainty for both external boundaries.

##### Why it happens now

Evidence schemas and recovery need to distinguish pre-dispatch failure, observed failure, cancellation, and unknown outcome.

##### Preconditions

Work waiting/current-attempt guards are defined.

##### Exact implementation work

- Model states: `requesting`, `streaming`, `completed`, `failed`, `cancelled_locally`, `provider_outcome_unknown`; enforce immutable terminal evidence and retry linkage.
- Tool states: `requested`, `dispatching`, `completed`, `interrupted_before_dispatch`, `outcome_unknown`; encode result-kind distinctions for validation, denial, file errors, exits, signals, timeout, cancellation, spawn, and cleanup.
- Require complete normalized model response before completion and dispatch eligibility.
- Require dispatch intent before Workstation invocation and observed cleanup before a cancelled tool can be completed definitively.
- Define legal attempt-to-work paired transitions.

##### Data/state introduced

Attempt transition decisions, result/certainty classifiers, retry-of relationships, and cleanup status values.

##### Contracts/interfaces introduced

Observed nonzero/timeout/error may still be a completed tool attempt; absence of terminal observation after dispatch is unknown, never failed.

##### Failure behavior

Any attempt outcome lacking required evidence is rejected. Duplicate provider call IDs or attempt ordinals are conflicts, not overwrites.

##### Validation

Exhaustive transition/evidence matrices and tests for every crash classification row in the architecture.

##### Exit criteria

- [ ] Intent and terminal evidence requirements are executable.
- [ ] Retry grouping does not reuse attempt identity.
- [ ] Tool outcomes cannot be silently retried.

##### What is deliberately NOT implemented yet

Provider billing reconciliation, remote execution deduplication, tool rollback, or exact provider cancellation guarantees.

#### Substage 4.3: Implement cancellation, recovery, and model terminal-decision functions

##### Objective

Centralize race and restart decisions that span the three lifecycles.

##### Why it happens now

Repositories will later persist these decisions atomically; leaving them to ad hoc service branches would create contradictory recovery.

##### Preconditions

Substages 4.1–4.2 are exhaustive.

##### Exact implementation work

- Define cancellation decisions at every checkpoint: before model, after provider, before tool requested/dispatch, while external wait, before next iteration, and before final commit.
- Define recovery classification from old runtime ownership/current attempt state exactly as the startup table specifies.
- Define ordered-model-output terminal decision for text, tools, structured data, refusal, incomplete/failed/empty/unknown items, and work cancellation.
- Define limit-exceeded terminal reasons for context, model attempts, loop steps, tool calls, output items/argument bytes, invocation time, and work time.
- Require synthetic unknown/interruption context status for later work as a recovery output, not an invented assistant message.

##### Data/state introduced

Pure `CancellationDecision`, `RecoveryClassification`, and `TerminalDecision` results with event/reason requirements.

##### Contracts/interfaces introduced

Durable cancellation wins over late provider output; unknown semantics fail closed; recovery never resumes an arbitrary old active loop in V0.

##### Failure behavior

An unclassifiable combination is an invariant failure that prevents readiness. Cleanup uncertainty maps to interrupted work.

##### Validation

Decision tables cover every architecture recovery row, mixed output order, refusal, empty output, late response/cancel races, and all limits.

##### Exit criteria

- [ ] Every old-runtime active shape has one deterministic outcome.
- [ ] Every terminal model response shape is classified.
- [ ] Cancellation cannot dispatch new work after winning.

##### What is deliberately NOT implemented yet

Automatic reconciliation/resume, human approval flows, provider-side cancellation polling, or UI rendering.

## Stage 5: SQLite adapter foundation, migrations, and startup lifecycle

### Objective

Create the persistence port, real file-backed SQLite adapter, migration/compatibility framework, required durability settings, single-instance guard, and integrity-check shell. Durable Craxii bootstrap waits until the core and journal schemas exist.

### Why it happens now

Domain transitions are stable; durable schema work can now implement them without leaking SQLx upward. All later commands, attempts, and replay depend on a correct database lifecycle.

### Preconditions

Stages 1–4 pass, including state-machine tests and validated configuration.

### Exact implementation work

- Execute Substages 5.1–5.3.
- Use real temporary database files for integration tests; reserve in-memory SQLite only for pure query experiments that make no WAL claim.
- Keep the service unready until migrations, integrity, bootstrap, and later recovery finish.

### Data/state introduced

Database file/WAL/SHM, schema-version history, startup lock, database-open classification, and persistence test fixtures.

### Contracts/interfaces introduced

Intent-specific `StateStore` operations, transaction context, write coordination, schema compatibility, and bootstrap/reopen lifecycle.

### Failure behavior

Migration, pragma, lock, integrity, busy-timeout, disk, or version failure keeps readiness false and produces a redacted storage/operator error. No partial service is exposed.

### Validation

Fresh migrate, idempotent reopen, forward-version refusal, WAL/FULL pragma checks on every connection, concurrent read/write tests, forced busy timeout, and process-reopen tests.

### Exit criteria

- [ ] SQLx is contained in `adapters/sqlite`.
- [ ] Required pragmas are applied and verified per connection.
- [ ] Forward-only migrations and max-schema checks work.
- [ ] Only one backend instance can own the local database.
- [ ] Empty/current/newer/inconsistent database states are classified safely before serving.

### What is deliberately NOT implemented yet

Domain tables, durable principal bootstrap, journal append, commands, scheduler, backups, remote databases, or automatic repair.

### Substages

#### Substage 5.1: Define the intent-specific State Store port and transaction vocabulary

##### Objective

Give application services narrow persistence operations without exposing SQL, rows, pools, or a generic repository.

##### Why it happens now

The SQLite adapter needs an inward-facing contract before schema-specific methods proliferate.

##### Preconditions

Domain transitions and records are stable enough to name intents.

##### Exact implementation work

- Define port operations for bootstrap/load identity, accept message/create work, claim next work, transition work/event, request/finish cancel, begin/finish invocation, request/dispatch/finish tool, commit final assistant completion, snapshot bootstrap, replay public journal candidates, recover old runtime, and verify consistency.
- Separate read snapshots from short write transactions.
- Represent expected state/version and resulting event offset ranges explicitly.
- Return domain records/errors, never SQLx rows/errors.
- Avoid a transaction object escaping application call scope; each intent owns its atomic boundary.

##### Data/state introduced

No persisted state; port input/output command types and transaction result receipts.

##### Contracts/interfaces introduced

Application decides semantic transitions; adapter enforces atomic persistence/constraints and returns committed facts.

##### Failure behavior

Conflict, busy, constraint, integrity, and I/O results are normalized without string parsing in application code.

##### Validation

Compile-time dependency tests and a fake store verifying application services call one intent rather than piecemeal CRUD.

##### Exit criteria

- [ ] Every architecture transaction has one named port operation.
- [ ] No generic update/delete-journal method exists.
- [ ] No SQLx type crosses the port.

##### What is deliberately NOT implemented yet

A universal unit-of-work abstraction, cross-database lowest-common-denominator API, or remote store implementation.

#### Substage 5.2: Build SQLx migration and connection infrastructure

##### Objective

Open SQLite with the exact durability/concurrency policy and apply compatible forward-only migrations.

##### Why it happens now

All durable tables and tests need one trustworthy connection factory and write coordinator.

##### Preconditions

Validated state paths and State Store port exist.

##### Exact implementation work

- Add SQLx SQLite support and versioned `backend/migrations/` harness.
- Create the database parent securely and open a small pool (default four) with per-connection `WAL`, `FULL`, foreign keys, 5-second busy timeout, memory temp store, and explicit 1000-page autocheckpoint.
- Add one in-process `WriteCoordinator`; acquire writer early with `BEGIN IMMEDIATE` semantics while keeping transactions short.
- Record/apply migrations before readiness; store/read schema version and binary maximum.
- Add checkpoint/WAL-size instrumentation hooks without tuning away durability.

##### Data/state introduced

Migration table, SQLite database/WAL/SHM files, pool, writer coordinator, and schema-version receipt.

##### Contracts/interfaces introduced

All connections have verified pragmas; no network filesystem is supported; write retries are allowed only before external effects.

##### Failure behavior

Pragma mismatch, newer schema, partial migration, busy timeout, or nonlocal/invalid path is fatal/unready. Destructive down migrations are unsupported.

##### Validation

Inspect pragmas on every pooled connection, migrate empty and already-current files, simulate newer schema, hold a writer to force timeout, checkpoint/reopen, and confirm committed WAL data survives process reopen.

##### Exit criteria

- [ ] WAL/FULL/foreign keys/busy timeout/temp store are verified.
- [ ] One writer policy is enforced.
- [ ] Migrations are forward-only and repeatable.
- [ ] Network paths and incompatible schemas fail closed.

##### What is deliberately NOT implemented yet

Performance pragmas, PostgreSQL, online multi-instance migration, automatic downgrade, or raw file-copy backup.

#### Substage 5.3: Implement single-instance startup and the integrity-check shell

##### Objective

Create a database lifecycle that safely gives one process ownership and classifies the database before domain bootstrap exists.

##### Why it happens now

Later schemas, bootstrap, and recovery need a safe exclusive-open and validation sequence.

##### Preconditions

Connection/migration infrastructure works on real files.

##### Exact implementation work

- Acquire an OS-level exclusive lock under the configured state lock directory before migrations/recovery and hold it for process lifetime.
- Implement `quick_check`, foreign-key check, schema compatibility, and an extensible application-invariant checker whose domain checks can be registered after their tables exist.
- Classify the opened database as empty schema, migrated-but-uninitialized, initialized-current, or incompatible without creating domain rows yet.
- Define the bootstrap-coordinator interface that Stage 7 will invoke after core and journal migrations exist; it must generate IDs once inside one transaction and load existing IDs on reopen.
- Keep readiness false and expose liveness only until the complete startup pipeline later includes recovery/scheduler/model/tool validation.

##### Data/state introduced

Exclusive lock handle and database lifecycle classification only.

##### Contracts/interfaces introduced

Only one process may proceed from validation toward later bootstrap/recovery; no identity decision occurs in this infrastructure stage.

##### Failure behavior

Second-instance lock, failed integrity check, unknown migration state, or invalid database classification is fatal and requires operator action.

##### Validation

Race two processes for the lock, open empty/current/newer-schema fixtures, corrupt a fixture deliberately, and verify readiness remains false.

##### Exit criteria

- [ ] A second backend cannot open the same state as owner.
- [ ] Empty versus initialized database status is explicit.
- [ ] Integrity failures never self-repair or regenerate identity.
- [ ] Startup ordering is executable and tested.

##### What is deliberately NOT implemented yet

Domain bootstrap/identity rows, machine replacement automation, external identity, automatic projection repair, scheduler start, or production readiness.

## Stage 6: Core durable schema and constraint spine

### Objective

Create the relational tables, keys, checks, and indexes for identity, conversation, messages, work, client commands, runtime ownership, workstations, and workspaces before any service writes them.

### Why it happens now

The SQLite lifecycle is trustworthy and domain invariants are executable. Journal and application transactions need stable relational targets next.

### Preconditions

Stage 5 passes on real file-backed databases.

### Exact implementation work

- Execute Substages 6.1–6.3 in migration order.
- Match the architecture's entities, relationships, uniqueness, and state constraints; deviations require an architecture change, not a convenient column omission.
- Add indexes from known queries, not speculative analytics.

### Data/state introduced

`craxii_principals`, `conversations`, `messages`, `work_items`, `client_devices`, `client_commands`, `runtime_instances`, `workstations`, and `workspaces`, plus their constraints/indexes. `work_item_inputs` waits for the journal table in Stage 7 so its event foreign key is real from first creation.

### Contracts/interfaces introduced

SQL row codecs contained in the adapter, guarded version updates, uniqueness for work ordinals/device commands/active work, and current-state query shapes.

### Failure behavior

Constraint or decode failures normalize to conflict/invariant/storage errors. The adapter never loosens a constraint or maps an unknown stored enum to a default.

### Validation

Migrate from empty, inspect table/index/foreign-key definitions, exercise every check/unique constraint, reopen under WAL, and prove row/domain round trips.

### Exit criteria

- [ ] All core current-state entities exist with architecture-required fields.
- [ ] Active-work and ordinal uniqueness are database-enforced.
- [ ] Client command response/idempotency material has durable storage.
- [ ] No bootstrap rows exist before Stage 7's journal-aware transaction.

### What is deliberately NOT implemented yet

Journal/input relations, evidence tables, repository business transactions, seed identity, scheduler logic, or general query APIs.

### Substages

#### Substage 6.1: Create identity, workstation, workspace, conversation, and runtime tables

##### Objective

Lay down the durable identity/topology tables in foreign-key-safe order.

##### Why it happens now

Messages, work, attempts, and journal events all reference these roots.

##### Preconditions

Migration harness and canonical row codecs are available.

##### Exact implementation work

- Create `craxii_principals` with immutable primary key, active lifecycle, display/owner fields, creation architecture revision, and nullable primary/default links populated during bootstrap.
- Create `workstations` with local kind, stable ID, positive generation, hosting evidence, architecture/OS/capabilities, and last-seen fields.
- Create `workspaces` with stable logical identity/name/root plus local resolved root as adapter evidence; enforce uniqueness per workstation.
- Create `conversations` with one primary kind per Craxii, positive `next_work_ordinal`, and optimistic state version.
- Create `runtime_instances` with boot ID, diagnostic PID, build/Git/schema metadata, workstation generation, lifecycle/heartbeat/terminal fields.
- Add explicit foreign keys and stored enum/check constraints; do not use physical instance/path values as keys.

##### Data/state introduced

Empty identity/topology/current-runtime tables and indexes.

##### Contracts/interfaces introduced

One V0 principal and one primary conversation are application invariants; workstation generation and runtime instance are distinct; an existing database's IDs outrank host discovery.

##### Failure behavior

Invalid enum/generation/state version, duplicate primary conversation/logical workspace, or dangling reference is rejected by SQLite and normalized.

##### Validation

Schema introspection, positive/negative inserts, runtime PID reuse fixtures showing ownership remains runtime-ID-based, and path/identity separation tests.

##### Exit criteria

- [ ] Root identity/topology tables have all normative fields.
- [ ] Required uniqueness/checks/foreign keys fire.
- [ ] No EC2/volume/PID/path column serves as a domain primary key.

##### What is deliberately NOT implemented yet

External control-plane identities, multiple workstation kinds, remote workspace attachment, or runtime recovery writes.

#### Substage 6.2: Create message, work, device, and command tables

##### Objective

Create the durable responsibility and idempotency projections used by the command/scheduler spine.

##### Why it happens now

Their parent identity/topology rows now have schema definitions, and the journal/input link will be added next.

##### Preconditions

Substage 6.1 migration succeeds.

##### Exact implementation work

- Create immutable `messages` with content JSON/hash, role, producer work, client device/message pairing, and committed time.
- Create `work_items` with kind/state/version, zero priority, conversation ordinal, workspace, runtime/current-attempt links, timestamps, terminal reason/detail, and correlation.
- Create `client_devices` with display name, unique SHA-256 token hash, lifecycle timestamps, and no raw token field.
- Create `client_commands` keyed by device/idempotency key with type, typed request hash, original HTTP status/body, committed cursor, and creation time.
- Add nullable-pair and state/timestamp checks where SQLite supports them; enforce immutable-message behavior through repository API and trigger only if it materially improves protection without obscuring migrations.

##### Data/state introduced

Empty message/work/device/idempotency tables and query indexes for conversation order, queued work, active work, and token lookup.

##### Contracts/interfaces introduced

The client-message uniqueness scope is `(device_id, client_message_id)`; command idempotency is `(device_id, idempotency_key)`; message and work are separate records.

##### Failure behavior

Duplicate identity/material conflicts are distinguishable from storage failure. Unknown stored work states or inconsistent timestamp fields fail decoding/integrity.

##### Validation

Insert/round-trip every role/state, reject partial client identity pairs, duplicate ordinals/devices/commands, invalid priority, and inconsistent terminal/current-attempt combinations.

##### Exit criteria

- [ ] Message/work separation is represented.
- [ ] Exact original command responses can be stored.
- [ ] Raw bearer tokens have no schema location.
- [ ] Queue and active-state queries are index-supported.

##### What is deliberately NOT implemented yet

Content attachments beyond text, command retention/pruning, priority scheduling, multiple work kinds, or protocol handlers.

#### Substage 6.3: Enforce concurrency constraints and prove core schema behavior

##### Objective

Make the database—not process convention—the final guard for one-active work and unique causal order.

##### Why it happens now

The tables exist, and journal/command code should be built against proven constraints rather than repairing them later.

##### Preconditions

Substages 6.1–6.2 are migrated.

##### Exact implementation work

- Add the partial unique index over `conversation_id` for `running`, `waiting_on_model`, `waiting_on_tool`, and `cancel_requested`.
- Enforce unique `(conversation_id, conversation_work_ordinal)` and positive ordinals.
- Add indexes for smallest queued ordinal, old-runtime nonterminal scans, current invocation/tool lookup, message order, token hash, and command lookup.
- Implement guarded row update helpers with expected state/version and exact affected-row checks, still internal to SQLite.
- Build reusable file-backed schema fixtures and transactional concurrency tests.

##### Data/state introduced

Constraint/index definitions and test fixture factories only.

##### Contracts/interfaces introduced

Concurrent writers may race, but one wins through SQLite constraints/guarded update and losers reload state.

##### Failure behavior

Constraint races return normalized state/idempotency conflict. Busy timeout remains a storage error; code never retries after an external effect.

##### Validation

Concurrent inserts/updates attempt duplicate ordinal, two active works, stale versions, and duplicate device commands; assert one winner and no partial rows after reopen.

##### Exit criteria

- [ ] One-active work is physically enforced.
- [ ] FIFO ordinals cannot duplicate.
- [ ] Guarded updates detect stale state.
- [ ] Query plans use intended indexes in representative fixtures.

##### What is deliberately NOT implemented yet

Distributed locks, database polling scheduler, event append, or priority/concurrency configuration.

## Stage 7: Journal, stream allocation, deterministic projection, and initial bootstrap

### Objective

Create the append-only event ledger, work-input relation, typed event taxonomy, transactional stream allocation, pure projector, consistency checks, and first durable Craxii bootstrap.

### Why it happens now

Core tables and state machines exist. All later business transactions require domain events and projections to commit together from their first write.

### Preconditions

Stages 5–6 pass and the database contains no product rows.

### Exact implementation work

- Execute Substages 7.1–7.3.
- Make generic event update/delete impossible through application repositories.
- Add journal/projection comparison to every later integration gate.

### Data/state introduced

`journal_events`, `stream_heads`, `work_item_inputs`, typed/versioned event payloads, global offsets/stream sequences, bootstrap identity/topology rows, and `craxii.initialized`/`conversation.created` events.

### Contracts/interfaces introduced

Transactional append with exact payload bytes/hash, causation/correlation, pure projection, and journal-aware bootstrap/reopen.

### Failure behavior

Unknown required event versions, stream gaps, payload hash mismatch, journal/projection disagreement, or invalid input correlation prevents readiness. No silent rebuild mutates production state.

### Validation

Append concurrency, stream allocation, event codec fixtures, bootstrap/reopen, pure replay comparison, corruption detection, and read-only enforcement tests.

### Exit criteria

- [ ] Journal offsets and per-stream sequences are durable and deterministic.
- [ ] Bootstrap creates one stable Craxii/conversation/workstation/workspace atomically with required events.
- [ ] Projector output equals stored projections.
- [ ] Historical rows have no application update/delete path.

### What is deliberately NOT implemented yet

Message commands, scheduler transitions, public event projection, journal pruning, hash chains, repair commands, or derived memory/search projections.

### Substages

#### Substage 7.1: Create journal/input schema and typed event codecs

##### Objective

Persist the normative event envelope and causal work-input relation without untyped decision logic.

##### Why it happens now

All future state-bearing mutations must append events from the outset.

##### Preconditions

Core parent tables exist and domain event names/versions are reviewed.

##### Exact implementation work

- Create `journal_events` with AUTOINCREMENT global offset, unique event ID, Craxii/stream/sequence/type/version, optional conversation/work/causation/runtime links, correlation/actor, exact payload JSON/hash, and timestamps.
- Create `stream_heads` and `work_item_inputs` with event foreign key, relationship, ordinal, actor/time, and per-work uniqueness.
- Define typed payload structures for every required event in the frozen taxonomy and an event registry declaring state-bearing versus non-state-bearing semantics.
- Serialize canonical stored payload bytes deterministically enough to hash/verify the exact stored representation; state decisions deserialize typed versions.
- Reject unknown version for required reconstruction and permit unknown types only when explicitly declared non-state-bearing.

##### Data/state introduced

Empty journal/stream/input tables and versioned event codecs.

##### Contracts/interfaces introduced

`journal_offset` is global client cursor; `stream_seq` is aggregate order; work inputs point to durable causal events, not messages by convention.

##### Failure behavior

Bad payload/version/hash/actor/stream form is an invariant/storage error. A payload cannot be partially decoded and defaulted.

##### Validation

Golden payload fixtures, version-compatibility tests, hash mismatch tests, foreign-key/input-correlation negatives, and safe 64-bit cursor serialization.

##### Exit criteria

- [ ] Every required event has a typed v1 payload.
- [ ] Event/input constraints match the architecture.
- [ ] Unknown state-bearing semantics fail closed.

##### What is deliberately NOT implemented yet

Public WebSocket event shapes, event retention, snapshots/epochs, or event-sourcing-only storage.

#### Substage 7.2: Implement transactional stream allocation, append, and pure projection

##### Objective

Guarantee append order and provide an independent deterministic reconstruction oracle.

##### Why it happens now

Bootstrap and all business transactions need atomic event allocation; projections need a correctness comparator before they grow.

##### Preconditions

Journal schema/codecs and state-machine transition decisions exist.

##### Exact implementation work

- Implement stream-head creation/increment under the same immediate write transaction; never use an unlocked `MAX + 1`.
- Append one or multiple events in caller-specified semantic order and return their offset range/IDs/sequences.
- Verify causation references and correlation/work/conversation consistency before insertion.
- Implement a pure projector that consumes ordered typed events and derives principal/conversation message order, work lifecycle/current attempt links, cancellation/interruption, attempt/artifact links, and unresolved unknown warnings.
- Implement a comparator that loads stored projections and reports exact discrepancies without mutating them.

##### Data/state introduced

Stream heads and journal rows when invoked; pure reconstructed state in tests/tools.

##### Contracts/interfaces introduced

Projection mutation and state-bearing event append occur in one transaction; the projector is side-effect-free and version-aware.

##### Failure behavior

Sequence conflicts roll back; projection mismatch is fatal at readiness; comparator output is redacted and diagnostic, not a repair.

##### Validation

Concurrent appenders on same/different streams, multi-event ordering, causation negatives, rollback tests, and replay fixtures for every lifecycle transition.

##### Exit criteria

- [ ] Stream sequences are positive/contiguous per stream under concurrency.
- [ ] Global offsets strictly increase and may safely contain gaps.
- [ ] Pure replay produces deterministic state.
- [ ] No generic journal mutation API exists.

##### What is deliberately NOT implemented yet

Cross-database global ordering, asynchronous projectors, event buses, or automatic projection replacement.

#### Substage 7.3: Commit initial Craxii bootstrap and complete startup integrity checks

##### Objective

Create the one local V0 identity and topology exactly once with journal evidence, then verify it on every reopen.

##### Why it happens now

Core and journal schemas plus transactional append now exist; no later service should operate without stable root IDs.

##### Preconditions

Substages 7.1–7.2 and Stage 5 startup lock are complete.

##### Exact implementation work

- In one write transaction on an uninitialized current schema, generate and insert the principal, primary conversation, local workstation generation, and default workspace; populate principal links.
- Append `craxii.initialized` and `conversation.created` in their proper streams with typed payloads/correlation. Do not invent events absent from the taxonomy for workstation/workspace creation; retain their canonical rows.
- Mark database initialization version so a crash rolls back or completes the whole seed without duplicate identities.
- On reopen, load existing IDs, compare configured logical workspace mapping and observed workstation capabilities, update only permitted last-seen evidence, and preserve identity.
- Extend integrity checks for one principal/primary conversation, stream-head maxima, active/current-attempt consistency, input correlation, terminal timestamps/cleared attempts, and referenced canonical artifacts when those exist.

##### Data/state introduced

The first canonical Craxii, conversation, local workstation, workspace, stream heads, and two initialization journal events.

##### Contracts/interfaces introduced

Database continuity defines V0 principal continuity; host replacement may change workstation generation but cannot regenerate Craxii.

##### Failure behavior

Crash before seed commit leaves no product rows; after commit reopen sees one complete bootstrap. Multiple/mismatched roots prevent readiness.

##### Validation

Kill before/after seed commit, race bootstrap attempts, reopen under changed diagnostic PID/hostname, replay initialization, and compare journal/projections.

##### Exit criteria

- [ ] Bootstrap is exactly-once and crash-atomic.
- [ ] Stable IDs survive reopen/move.
- [ ] Quick/foreign-key/application/projector checks pass.
- [ ] Identity is not derived from provider/host/client state.

##### What is deliberately NOT implemented yet

Device enrollment, machine replacement workflow, external durable identity, or public bootstrap API.

## Stage 8: Evidence-attempt schema and local artifact store

### Objective

Create durable context/model/tool/artifact records and a content-addressed local evidence store with rename-before-reference, bounded capture, provenance, and orphan handling.

### Why it happens now

Commands and scheduling can be built without external calls, but model/tool stages require complete evidence schema and artifact atomicity first.

### Preconditions

Stage 7 establishes stable work/runtime/workstation/workspace/journal roots and transaction helpers.

### Exact implementation work

- Execute Substages 8.1–8.3.
- Treat artifact bytes and metadata as one commit protocol, not ordinary workspace files.
- Introduce artifact failpoints now, before tool/process code relies on them.

### Data/state introduced

`context_manifests`, `context_manifest_sources`, `model_invocations`, `tool_executions`, `artifacts`, local `tmp/sha256` layout, retention classes, hashes/sizes/truncation/provenance, and orphan observations.

### Contracts/interfaces introduced

`ArtifactStore`, attempt repository row codecs, evidence finalization handles, bounded inline/model/client projections, and artifact-reference transaction rules.

### Failure behavior

Write/hash/fsync/rename/metadata failure prevents any terminal record from referencing missing bytes. Post-rename/pre-DB crash leaves a recoverable orphan, never a false durable reference.

### Validation

Migration/constraint tests, binary/large-stream artifact tests, concurrent same-content writes, fsync/rename failure injection, orphan grace cleanup, and full hash verification.

### Exit criteria

- [ ] All architecture-required attempt/evidence fields are durable.
- [ ] Artifact IDs/keys hide physical public paths.
- [ ] Canonical evidence is finalized before DB reference.
- [ ] Output limits and observed/captured/inline counts remain distinct.

### What is deliberately NOT implemented yet

Provider/tool services, S3, automatic retention deletion, artifact HTTP download, workspace backup, or unbounded logs.

### Substages

#### Substage 8.1: Create context, model, tool, and artifact migrations

##### Objective

Persist every attempt/provenance/measurement field required by the architecture before external action begins.

##### Why it happens now

Later services must not ship with “temporary” incomplete attempt rows that require destructive migration.

##### Preconditions

All referenced core/journal IDs and enums exist.

##### Exact implementation work

- Create context manifest/source tables with target/config/prompt/toolset/policy versions, eligibility cutoff, source identity XOR checks, hashes, byte/token/limit/utilization/omission fields, and optional request artifact.
- Create model invocations with logical/attempt grouping, step ordering, context/runtime ownership, target/selection/capability/options snapshots, state, request/response evidence, ordered normalized output, provider IDs/timestamps/usage/stop/tool/draft/error fields.
- Create tool executions with source invocation/call/step/ordinal/tool/schema, arguments/hashes, workstation generation/workspace/cwd/privilege/authority/timeout/output/execution IDs, lifecycle times, structured results, artifact links, byte/truncation/cleanup/error evidence.
- Create artifacts with stable ID, backend/key/hash/length/observed length/type/encoding/name/retention/truncation/compression/producers and uniqueness constraints.
- Add legal-state/check/index constraints and foreign-key check the completed migration.

##### Data/state introduced

Empty detailed evidence tables and supporting indexes.

##### Contracts/interfaces introduced

Each provider retry is a row; each tool call ordinal is unique; one stable execution ID identifies dispatch; terminal evidence is immutable.

##### Failure behavior

Unknown state/result/retention, duplicate attempt/call/execution, invalid source identity, or incomplete terminal fields is rejected.

##### Validation

Round-trip every lifecycle/result variant; negative constraint matrix; foreign-key check; migration/reopen from prior schema.

##### Exit criteria

- [ ] Schema covers every normative field/relationship.
- [ ] Retry/tool order uniqueness is enforced.
- [ ] Context sources have exactly one source identity.
- [ ] No provider wire object is stored as a canonical typed contract.

##### What is deliberately NOT implemented yet

Cost tables, searchable raw output, automatic analytics aggregation, provider conversations, or tool stdout rows per chunk.

#### Substage 8.2: Implement ArtifactStore and atomic content-addressed writes

##### Objective

Store bounded immutable evidence bytes safely on the same local filesystem as their final artifact path.

##### Why it happens now

Process/provider capture must use this protocol from first implementation; retroactive artifact promotion would lose crash ordering.

##### Preconditions

Validated artifact root and artifact metadata types exist.

##### Exact implementation work

- Define `ArtifactStore` operations for start/write/finalize/verify/open metadata-safe bytes/orphan scan; keep storage keys opaque above the adapter.
- Create restrictive `tmp/` and sharded `sha256/<prefix>/<hash>` directories on one filesystem.
- Stream to unique partial files while hashing/counting and enforcing hard capture limits; fsync canonical evidence files, atomically rename, and sync parent directory per chosen filesystem policy.
- Handle content-address collisions by verifying existing hash/length rather than overwriting blindly.
- Return a finalized descriptor that can be inserted with its referencing terminal record in one later DB transaction.

##### Data/state introduced

Partial files, immutable hash-keyed objects, finalized descriptors, and write timing/count evidence.

##### Contracts/interfaces introduced

Finalize-before-reference; a finalized unreferenced object is only an orphan candidate; storage path never enters model/client contracts.

##### Failure behavior

Limit/disk/fsync/hash/rename/collision mismatch returns `artifact_error`; partial files are closed and retained/cleaned safely, and no terminal outcome is fabricated.

##### Validation

Small/empty/binary/limit writes, concurrent identical/different content, injected short write/fsync/rename error, file mode, hash verification, and restart behavior.

##### Exit criteria

- [ ] Canonical writes are content-addressed and atomic.
- [ ] Existing objects are verified before reuse.
- [ ] Final descriptors contain all DB metadata without exposing paths.
- [ ] Capture bounds apply while drains may continue counting externally.

##### What is deliberately NOT implemented yet

Remote/object storage, encryption separate from EBS, compression by default, deletion/lifecycle policies, or client file browsing.

#### Substage 8.3: Integrate metadata transactions, output policy, orphan cleanup, and crash tests

##### Objective

Close the gap between artifact bytes and detailed records while defining exact capture/inline/context/client limits.

##### Why it happens now

Tools and models need one evidence policy and proven crash behavior before they emit output.

##### Preconditions

Substages 8.1–8.2 and journal append transactions exist.

##### Exact implementation work

- Implement transaction helpers that insert/reuse artifact metadata and reference it from one terminal attempt plus optional `artifact.recorded` event after rename.
- Encode defaults: 8 MiB per stdout/stderr artifact, 64 KiB combined model result, 32 KiB per-stream head/tail projection, 256 KiB durable WebSocket payload, and 64 KiB user text; keep configurable values within hard bounds.
- Record observed, captured, returned/inline, and omitted bytes separately; preserve stdout/stderr as independent streams.
- Implement orphan scan after startup recovery: ignore young partial/final objects, query references, report counts, and delete only unreferenced objects past a conservative grace period through an explicit maintenance operation.
- Activate `after_artifact_rename_before_db_commit` and pre-rename failure hooks; prove no committed missing reference and safe orphan detection.

##### Data/state introduced

Artifact metadata rows/references/events when used, truncation policies, orphan observations, and maintenance diagnostics.

##### Contracts/interfaces introduced

Model-facing truncation is explicit; raw evidence and model projection are different artifacts/views; canonical evidence retention is backup-required.

##### Failure behavior

DB commit failure after rename creates an orphan and leaves attempt nonterminal/unknown according to its external-action phase. Cleanup never deletes referenced or too-recent objects.

##### Validation

Crash at every write/rename/metadata boundary, compare DB/files, test head/tail math and invalid UTF-8 display marking, and run full hash/integrity checks.

##### Exit criteria

- [ ] Rename-before-reference invariant is crash-proven.
- [ ] All four output limits are distinct and tested.
- [ ] Orphan cleanup is conservative and observable.
- [ ] Artifact evidence participates in projector/integrity checks.

##### What is deliberately NOT implemented yet

Automatic production pruning, S3 replication, tool/provider capture itself, or exposing canonical artifacts to the model without bounded projection.

## Stage 9: Device authentication and idempotent Command Service

### Objective

Provision a V0 client device securely and implement atomic message acceptance, exact duplicate/conflict semantics, and durable cancellation commands independently of HTTP transport.

### Why it happens now

Identity, core/journal/evidence schema, and transactions are ready. Durable responsibility must be correct before scheduling, models, or clients.

### Preconditions

Stages 6–8 pass, one Craxii/conversation/workspace is bootstrapped, and State Store append operations exist.

### Exact implementation work

- Execute Substages 9.1–9.3.
- Keep device provisioning an offline operator action; no public enrollment endpoint.
- Add command-level telemetry without content/token leakage.

### Data/state introduced

Provisioned `client_devices`, `client_commands`, user messages, queued work, trigger inputs, command responses, message/work/cancellation events, allocated work ordinals, and dedup/conflict measurements.

### Contracts/interfaces introduced

`DeviceAuthenticator`, request canonicalizer/hash, `CommandService.accept_message`, and `CommandService.cancel_work` with exact committed receipts.

### Failure behavior

Unauthenticated/revoked/invalid commands create no domain state. Exact retries return original logical results; same key/different material conflicts with no write.

### Validation

Offline token tests, constant-time comparison checks, transaction rollback injection, concurrent duplicate and ordinal allocation tests, lost-response simulation, cancellation race matrices, and journal/projection comparison.

### Exit criteria

- [ ] One new message creates exactly one work atomically.
- [ ] Duplicate retries cannot duplicate rows/events.
- [ ] Conflict/revocation/validation paths have zero domain writes.
- [ ] Cancellation is a control command and never creates conversational work.

### What is deliberately NOT implemented yet

HTTP endpoints, WebSocket delivery, account auth/enrollment UI, scheduler execution, device recovery, or multiple-user authorization.

### Substages

#### Substage 9.1: Implement offline device provisioning and authentication

##### Objective

Create and resolve a random per-device bearer credential without storing or logging raw token material.

##### Why it happens now

Command identity/idempotency must be scoped to an authenticated device from first use.

##### Preconditions

Device schema, secret wrappers, secure random generation, and operator config are available.

##### Exact implementation work

- Add an offline local admin command or narrowly scoped provisioning tool that generates/accepts a random 256-bit token, prints it once to the operator through a safe channel, stores only SHA-256 plus device ID/display/timestamp, and supports revocation/listing without revealing tokens.
- Implement bearer parsing, hashing, constant-time digest comparison, revoked-state handling, and last-seen updates that do not delay command correctness.
- Ensure auth headers/tokens never enter normal tracing, errors, command hashes, journal, artifacts, or child environments.
- Create deterministic authentication fixtures using sentinel tokens.

##### Data/state introduced

One or more device rows and operator-held raw token; only the raw token later enters macOS Keychain.

##### Contracts/interfaces introduced

Successful authentication yields `device_id`; raw credential remains transport/bootstrap-only and never crosses into domain work.

##### Failure behavior

Missing/malformed/unknown/revoked tokens return uniform `401` semantics with no state-bearing event and no oracle about which part failed.

##### Validation

Provision/auth/revoke cycles, hash uniqueness, timing-safe comparator use review, sentinel log/database scan, and concurrent last-seen updates.

##### Exit criteria

- [ ] Raw token is displayed once and absent server-side afterward.
- [ ] Revocation takes effect for new requests/upgrades.
- [ ] Auth output is only a durable device identity.

##### What is deliberately NOT implemented yet

User accounts, OIDC/passkeys, enrollment UI, token recovery, device-to-device sync, or provider credentials.

#### Substage 9.2: Implement canonical request hashing and atomic message acceptance

##### Objective

Turn one authenticated V0 message command into exactly one durable responsibility and replayable receipt.

##### Why it happens now

All referenced tables/events/constraints exist, and no scheduler can race until acceptance semantics are proven.

##### Preconditions

Authenticated device, primary conversation/workspace, journal append, and write coordinator exist.

##### Exact implementation work

- Canonicalize the length-prefixed tuple of protocol version, command type, conversation ID, client message ID, and normalized ordered text blocks; require idempotency key equals client message ID.
- Pre-generate message/work/event/correlation IDs and allocate the conversation work ordinal inside one immediate transaction.
- Insert immutable user message; append `message.accepted`; insert queued work; insert trigger `work_item_input`; append caused `work.queued`; increment conversation ordinal/version; build exact response/status/cursor; insert `client_commands`; commit.
- On an existing device/key, compare type/hash and return stored IDs/status/body/cursor or conflict. Handle concurrent insert loser by reloading the winner.
- Notify scheduler/event delivery only after commit as lossy wakeup hints.

##### Data/state introduced

One user message, acceptance event, queued work, trigger input, queue event, updated conversation ordinal/version, and command receipt per genuinely new command.

##### Contracts/interfaces introduced

No committed message exists without its work/trigger/receipt; notification loss cannot lose work; response loss is safe to retry.

##### Failure behavior

Any validation/storage/event/constraint failure rolls back all effects. Same key/different material is definite conflict. Commit success followed by transport loss still returns original receipt later.

##### Validation

Fail before each insert and before/after commit, inspect zero-or-complete state, run simultaneous exact duplicates/conflicts, allocate many concurrent ordinals, and compare projector/current state.

##### Exit criteria

- [ ] Atomic one-message-one-work invariant is crash/concurrency proven.
- [ ] Exact duplicate returns original logical result.
- [ ] Events are emitted only for new domain transitions.
- [ ] `committed_cursor` is the command transaction's highest offset.

##### What is deliberately NOT implemented yet

Attachments, steering, command priority, natural-language routing, synchronous execution, or WebSocket submission.

#### Substage 9.3: Implement idempotent cancellation commands

##### Objective

Persist cancellation intent safely for queued, active, terminal, unknown, and duplicate targets.

##### Why it happens now

The work state machine and command receipt infrastructure exist; scheduler/runtime cleanup can consume this durable intent next.

##### Preconditions

Substage 9.2 command idempotency and Stage 4 cancellation decisions pass.

##### Exact implementation work

- Canonicalize a fresh cancel command ID, target work, protocol version, and command type into request material.
- For queued work, atomically transition to `cancelled`, append terminal cancellation event, store response/cursor, and ensure it cannot be claimed.
- For active work, atomically transition to `cancel_requested`, append event, store `202` cleanup-pending response, and emit post-commit cancellation wakeup.
- For terminal work, return its existing terminal state as an idempotent no-op while still persisting the new command receipt only if required by command semantics; append no false transition.
- Return `404` unknown target and `409` key conflict with no domain transition; define whether rejected/no-op command receipts persist consistently with the architecture's client-command rules.

##### Data/state introduced

Cancellation command receipts and, when state changes, cancel-requested/cancelled work projection plus journal event.

##### Contracts/interfaces introduced

Command acceptance and cleanup completion are separate for active work; terminal cancellation means cleanup confirmed or external activity absent.

##### Failure behavior

State-version races reload the winner. Cancellation never fabricates process cleanup or changes an already terminal reason.

##### Validation

Queued/each-active/each-terminal/unknown target tests, duplicate/conflict/lost response, simultaneous claim/cancel races, and zero conversational-work assertions.

##### Exit criteria

- [ ] Queued cancellation is terminal and unclaimable.
- [ ] Active cancellation is durable but nonterminal until cleanup.
- [ ] Duplicate/no-op/conflict semantics are exact.
- [ ] Every state change has one event and committed cursor.

##### What is deliberately NOT implemented yet

Signal delivery, provider cancellation, client cancel button, mass cancellation, or automatic retry/continue.

## Stage 10: Durable scheduler, cancellation coordination, shutdown, and startup recovery

### Objective

Claim FIFO work from SQLite with one-active enforcement, own all execution tasks, coordinate durable cancellation, shut down honestly, and classify every old-runtime incomplete attempt before readiness.

### Why it happens now

Commands can create/cancel durable work and all attempt tables exist. Execution ownership/recovery must be correct before the agent loop or real side effects are attached.

### Preconditions

Stages 7–9 pass, including concurrency/crash tests and runtime tables/events.

### Exact implementation work

- Execute Substages 10.1–10.4.
- Test scheduling initially with an explicit test `WorkRunner`; production readiness remains false until Stage 17 installs the real agent loop.
- Activate message/claim/cancel/shutdown failpoints at their exact boundaries.

### Data/state introduced

Runtime instance/heartbeat/stopping/recovery rows/events, claimed work ownership/start events, in-memory wakeups/task collection/cancellation tokens, durable cancellation completions, interruption/unknown classifications, and recovery counts.

### Contracts/interfaces introduced

`Scheduler`, owned `WorkRunner` boundary, `CancellationCoordinator`, graceful-shutdown protocol, and `RecoveryService` using intent-specific State Store transactions.

### Failure behavior

Lost wakeups only delay scans. Task panic is observed and forces classification/health degradation. Cleanup uncertainty becomes interrupted. Recovery inconsistency prevents readiness and scheduling.

### Validation

FIFO/conversation concurrency tests, fake runner ownership, cancel races, graceful timeout, SIGKILL/reopen across every preexisting attempt state, exact-once recovery, and journal/projection comparison.

### Exit criteria

- [ ] SQLite is the queue; notification loss cannot lose work.
- [ ] At most one work per conversation is active under concurrency.
- [ ] All tasks are owned/joined and panics observed.
- [ ] Old-runtime nonterminal state is classified exactly once before readiness.
- [ ] No ambiguous attempt is automatically called again.

### What is deliberately NOT implemented yet

Real agent execution, priority/global concurrency, steering, background triggers, arbitrary in-flight resume, distributed leasing, or production readiness.

### Substages

#### Substage 10.1: Implement FIFO claim transactions and scheduler task ownership

##### Objective

Move the next eligible durable work item into a live owned task without making memory authoritative.

##### Why it happens now

Queued work and database constraints are proven; the agent loop later needs one reliable owner.

##### Preconditions

Message acceptance creates valid ordered queued work and runtime identity can be persisted.

##### Exact implementation work

- Query conversations with queued work and no active row; within a short immediate transaction select smallest ordinal, guarded-update queued to running with current runtime/start time/version, and append `work.started`.
- Commit before spawning; if spawn fails, classify the now-active work through an explicit failure/interruption transaction rather than leaving it invisible.
- Implement periodic database scans plus lossy notify wakeups; scan on startup and after every terminal task.
- Own per-work tasks in a `JoinSet` or equivalent; map task result/panic to scheduler action and tracing.
- Use a test-only runner for controlled blocking/completion; do not let it enter production composition.

##### Data/state introduced

Work runtime ownership/start timestamp/version and start event; ephemeral scheduler scan state/tasks/wakeups.

##### Contracts/interfaces introduced

Claim is the only queued-to-running path; task starts only after commit; a task never proves work existence.

##### Failure behavior

Zero-row guarded update reloads. Spawn/panic/store failure is observed and health/recovery policy runs. Lost notify is repaired by next scan.

##### Validation

Multiple queued conversations, multiple scheduler contenders, one conversation with many ordinals, lost notifications, task panic, and service restart tests.

##### Exit criteria

- [ ] FIFO and one-active invariants hold under races.
- [ ] No task is detached.
- [ ] Claimed ownership references the current runtime.
- [ ] Scheduler can drain queued fixtures after earlier terminal work.

##### What is deliberately NOT implemented yet

Parallel work within one conversation, priority, distributed claims/leases, or real model/tool work.

#### Substage 10.2: Implement runtime cancellation coordination and graceful shutdown

##### Objective

Translate durable cancellation/shutdown into cooperative runtime checkpoints while requiring confirmed cleanup for `cancelled`.

##### Why it happens now

Scheduler owns tasks and Command Service emits cancel wakeups; external adapters will later plug into the same coordinator.

##### Preconditions

Substage 10.1 and cancellation command semantics exist.

##### Exact implementation work

- Maintain ephemeral work cancellation tokens keyed by stable work ID and linked to scheduler-owned tasks; always reload durable state at critical checkpoints.
- On cancel wakeup/poll, signal the owned runner; expose sub-cancellation handles to provider/workstation ports later.
- Complete queued/active cancellation through State Store only after the runner reports no activity or confirmed cleanup; append `work.cancelled` and clear ownership.
- On SIGTERM: mark unready/draining, stop claims, append `runtime.stopping`, cancel owned tasks, wait bounded configured grace, persist cancelled where confirmed and interrupted otherwise, join tasks, close server/pool cleanly.
- Ensure a late normal result cannot overwrite durable `cancel_requested`.

##### Data/state introduced

Ephemeral cancellation-token registry, durable terminal cancellation/interruption events, runtime stopping metadata, and cancellation latency measurements.

##### Contracts/interfaces introduced

Dropped futures are never cancellation; durable state wins races; shutdown and user cancellation share cleanup primitives but retain distinct reasons.

##### Failure behavior

Unconfirmed runner/provider/tool stop maps to interrupted/unknown as appropriate. Shutdown timeout exits nonzero only after best-effort classification; SIGKILL leaves recovery to next runtime.

##### Validation

Cancel before/after claim, late runner completion, simultaneous terminal/cancel, grace expiry, repeated SIGTERM, and owned-task leak checks.

##### Exit criteria

- [ ] No further step starts after cancellation wins.
- [ ] `cancelled` always means confirmed absence/cleanup.
- [ ] Graceful shutdown stops claims and observes every task.
- [ ] Uncertain cleanup is never reported as cancelled.

##### What is deliberately NOT implemented yet

Actual HTTP request cancellation, process signals, provider guarantees, pause/resume, or user approval.

#### Substage 10.3: Implement runtime instances, heartbeats, and deterministic startup recovery

##### Objective

Identify each process lifetime and convert old-runtime active state into honest terminal classifications before scheduling.

##### Why it happens now

Attempt schemas, state decisions, and journal transactions exist; recovery must precede any real external action.

##### Preconditions

Startup lock/migrations/integrity/bootstrap pass and scheduler remains stopped.

##### Exact implementation work

- Read Linux boot ID/diagnostic PID/build/schema/workstation generation and create a new runtime row plus `runtime.started` after initial integrity checks.
- Heartbeat at a bounded cadence without making heartbeat freshness the sole liveness proof.
- Scan every nonterminal work/model/tool attempt owned by another runtime; apply the exact recovery matrix in one or more short, deterministic transactions.
- Preserve queued work; mark active/no-attempt work interrupted; model requesting/streaming unknown; tool requested interrupted-before-dispatch; dispatching outcome unknown; cancel-requested unconfirmed interrupted; reconcile already-terminal rows only from committed evidence or fail readiness.
- Abandon ephemeral draft identity in recovery summary; append `runtime.recovery_performed` with redacted counts/duration/orphans/cleanup checks and start scheduler only afterward.

##### Data/state introduced

New runtime row/events, old runtime terminal annotations, recovered work/model/tool states/events, and recovery measurements.

##### Contracts/interfaces introduced

Runtime ownership is stable ID-based; recovery is idempotent and never invokes provider/Workstation; queued work remains eligible after interrupted predecessor becomes terminal.

##### Failure behavior

Unclassifiable or journal/projection-inconsistent state keeps service unready/fatal. Recovery transaction failure is retried only before readiness and no external action.

##### Validation

Fixture every recovery-table row, restart twice to prove exactly-once classification, preserve queued followers, assert no adapter calls, and compare journal/projector/projections.

##### Exit criteria

- [ ] Every old nonterminal state has one tested classification.
- [ ] New runtime/recovery events precede readiness/scheduling.
- [ ] Ambiguous tools become unknown and owning work interrupted.
- [ ] Second recovery does not append duplicate classifications.

##### What is deliberately NOT implemented yet

Automatic provider/tool resumption, remote process inspection after process death, or reconciliation of external systems.

#### Substage 10.4: Activate and pass early command/scheduler/recovery crash windows

##### Objective

Prove the durable responsibility spine around message commit, claim, cancel, and graceful shutdown before adding models/tools.

##### Why it happens now

These are the first complete crash boundaries supported by the subprocess controller.

##### Preconditions

Substages 10.1–10.3 and Stage 2 failpoint controller work.

##### Exact implementation work

- Activate `after_message_transaction_commit`, `after_work_claim_commit`, `after_cancel_requested_commit`, and `during_graceful_shutdown` with documented physical semantics.
- Add pre-commit companion hooks to prove rollback even though only post-commit names are normative.
- For each, launch service/test runner, reach marker, kill, reopen/recover, retry original command, and drain eligible queued work with the test runner.
- Assert quick/foreign-key/projector/invariant checks; exact row/event counts; one recovery classification; no duplicate work; correct committed cursor/replay candidates; no leaked task.

##### Data/state introduced

Crash-test databases/logs/evidence outside canonical product state; recovered runtime events in fixtures.

##### Contracts/interfaces introduced

Each failpoint has an expected durable-state oracle and can be reused in the final systemd suite.

##### Failure behavior

Any ambiguous assertion or timing-only crash is a test failure. The suite never deletes evidence until diagnosis is complete.

##### Validation

Run every failpoint repeatedly and in randomized test ordering; verify release build cannot activate them.

##### Exit criteria

- [ ] All four early crash windows pass deterministically.
- [ ] Duplicate retry after lost commit response is exact.
- [ ] Claim/cancel/shutdown cannot leave contradictory work state.
- [ ] Recovery remains adapter-free.

##### What is deliberately NOT implemented yet

Model/context/tool/artifact/final-answer failpoints, systemd cgroup behavior, or broad chaos/fuzz campaigns.

## Stage 11: Headless HTTP protocol, bootstrap snapshot, and durable replay

### Objective

Expose the proven command/query semantics through authenticated protocol v1 HTTP and a durable cursor-based WebSocket replay/live handoff, without ephemeral model drafts.

### Why it happens now

Commands, recovery, and journal order are executable headlessly. The backend contract must stabilize before model streaming or a native client depends on it.

### Preconditions

Stages 9–10 pass; scheduler can use a test runner; public event payload decisions are reviewed for redaction.

### Exact implementation work

- Execute Substages 11.1–11.4.
- Keep Axum/Tower handlers thin: authenticate/decode/delegate/map results.
- Use SQLite reads after a durable cursor for gap repair; never make broadcast delivery authoritative.

### Data/state introduced

No new canonical entity. Protocol request/response/event envelopes, request IDs, bootstrap snapshots, connection cursors, bounded send queues, and protocol measurements are introduced.

### Contracts/interfaces introduced

`GET /health/live`, restricted readiness, `GET /v1/bootstrap`, message/cancel HTTP commands, and `/v1/events?after=` WebSocket replay/live delivery with protocol version 1.

### Failure behavior

Invalid auth/version/body/cursor returns stable safe errors and no domain writes. Slow clients are disconnected with a replayable cursor; committed facts are never silently dropped.

### Validation

Transport contract fixtures, auth/body/version tests, lost HTTP response retry, snapshot/high-water races, replay from every offset, notification loss, skipped internal events, slow-client closure, and reconnect.

### Exit criteria

- [ ] Command acknowledgement is independent of WebSocket state.
- [ ] Bootstrap is one SQLite snapshot with a trustworthy high-water cursor.
- [ ] Durable event delivery converges after notification loss/disconnect.
- [ ] Provider/internal/secret detail cannot cross the public protocol.

### What is deliberately NOT implemented yet

Draft deltas, tool/model live streams, artifact downloads, admin endpoints, history pruning/cursor epochs, native client, or public readiness detail.

### Substages

#### Substage 11.1: Define and freeze protocol v1 types and endpoint behavior

##### Objective

Create Craxii-owned, versioned JSON contracts independent of Axum, provider, and database types.

##### Why it happens now

The application semantics are known and can be projected without guessing future model/tool behavior.

##### Preconditions

Canonical IDs/content/errors and Command Service receipts exist.

##### Exact implementation work

- Define request/response types for message and cancel commands, bootstrap projection, durable event envelope, future ephemeral envelope, and safe error envelope with `protocol_version=1`.
- Specify HTTP status behavior, required `Idempotency-Key`, UUID/cursor/timestamp representation, maximum payload sizes, additive optional-field policy, and unknown-version rejection.
- Define client-safe work/message/tool summary projections including queued/running/waiting/cancelled/failed/interrupted and `outcome_unknown` warnings.
- Map internal event types to allowed public names/payloads; document omitted internal types and cursor jumps.
- Produce language-neutral JSON fixtures for later Swift tests.

##### Data/state introduced

Versioned public data structures and golden fixtures only.

##### Contracts/interfaces introduced

Public protocol semantics belong to Craxii and remain stable across OpenAI/SQLite/Linux changes.

##### Failure behavior

Unknown required fields/types/version, mismatched idempotency header, unsafe integer, or oversized content fails before Command Service.

##### Validation

Golden encode/decode, unknown/additive field behavior, stable error mapping, cursor extremes, content limits, and internal-detail leakage tests.

##### Exit criteria

- [ ] Every endpoint/event has a v1 fixture.
- [ ] Internal and public event taxonomies are explicitly mapped.
- [ ] Swift-compatible numeric/string forms are fixed.

##### What is deliberately NOT implemented yet

OpenAPI code generation as a runtime dependency, provider events, user accounts, multi-conversation management, or binary attachments.

#### Substage 11.2: Implement Axum/Tower HTTP adapters and health behavior

##### Objective

Expose liveness, authenticated bootstrap/message/cancel calls, request limits, and safe status mapping.

##### Why it happens now

Protocol types and application services are ready; transport can remain purely delegating.

##### Preconditions

Substage 11.1 and device authentication/Command Service exist.

##### Exact implementation work

- Compose Axum routes at the specified paths and loopback/default local bind.
- Add request ID, bearer authentication, JSON content-type/body limits, command-only timeouts/concurrency limits, trusted-forwarded-header policy, and redacted tracing.
- Make liveness unauthenticated/minimal; keep readiness restricted and false until recovery, scheduler, registry, and default target are usable.
- Map normalized errors/statuses without stack/body/provider/database leakage.
- Notify scheduler/event delivery only from post-commit Command Service receipts.

##### Data/state introduced

Ephemeral listener/request state and request/protocol measurements; command calls create only the already-defined canonical state.

##### Contracts/interfaces introduced

Transport cannot execute work inline or hold command responses open for background execution.

##### Failure behavior

Middleware rejection creates no domain write. Timeout after a successful commit may lose HTTP response; same idempotency key returns the stored receipt.

##### Validation

In-process HTTP tests for every status/auth/body/version/content-type case, deliberate response loss, readiness transitions, and forwarded-header trust only from loopback.

##### Exit criteria

- [ ] All authoritative mutations use HTTP.
- [ ] Handlers contain no SQL/scheduler/model/tool behavior.
- [ ] Liveness/readiness claims are truthful.
- [ ] Request bodies/auth values never appear in normal traces.

##### What is deliberately NOT implemented yet

TLS in Rust, Web UI, long-lived command responses, generic admin APIs, or Caddy deployment.

#### Substage 11.3: Implement atomic bootstrap snapshot and public durable-event query

##### Objective

Give clients a consistent current projection plus a high-water cursor and a query that can replay all subsequent public facts.

##### Why it happens now

HTTP commands generate committed journal facts; reconnect correctness must precede live delivery.

##### Preconditions

Public projections, journal/query indexes, and file-backed snapshot tests exist.

##### Exact implementation work

- In one bounded SQLite read transaction, read head `H`, principal/conversation/messages, queued/active/recent terminal work, and unresolved interruptions/unknown outcomes; release before returning.
- Implement ordered journal query `after < offset <= high_water`, translating only client-visible events while tracking the high-water even across omitted internal rows.
- Ensure snapshot state contains no projection commit newer than `H` and no provider request/raw output/internal payload.
- Bound history/bootstrap size under V0 limits and fail explicitly rather than return a partial undocumented snapshot.
- Add query/page internals if needed, while presenting one ordered stream to the connection.

##### Data/state introduced

Ephemeral bootstrap/public-event projections and query timing/count metrics.

##### Contracts/interfaces introduced

`snapshot_cursor` is the maximum journal offset within the same read snapshot; a replay cursor may jump over internal events.

##### Failure behavior

Snapshot inconsistency or unknown required event version is an invariant/server error; service does not return a cursor paired with partial state.

##### Validation

Concurrent commit during bootstrap, commits before/after head read, replay at zero/head/ahead, internal event gaps, large history limit, and read-transaction release/checkpoint tests.

##### Exit criteria

- [ ] Snapshot/high-water race is closed.
- [ ] Public replay is strictly offset ordered.
- [ ] Internal omissions advance connection cursor safely.
- [ ] Long reads do not pin WAL indefinitely.

##### What is deliberately NOT implemented yet

Journal pruning, pagination exposed as product semantics, snapshot epochs, artifact payloads, or local client caching.

#### Substage 11.4: Implement durable WebSocket replay/live handoff and backpressure

##### Objective

Deliver committed events reliably across replay, notification loss, disconnect, and slow clients before drafts complicate the channel.

##### Why it happens now

Bootstrap/event queries are correct, so WebSocket can be a replaceable delivery adapter rather than a state substrate.

##### Preconditions

Substages 11.2–11.3 and post-commit notification channel exist.

##### Exact implementation work

- Authenticate the WebSocket upgrade and validate nonnegative/not-ahead `after` cursor.
- Subscribe to bounded commit notifications before reading replay high-water `R`; send all public events `(after,R]`, set `last_sent_cursor=R` even across omitted rows, discard notifications `<=R`, emit `sync.complete`, then enter live mode.
- On each wakeup, timeout poll, or broadcast gap, query SQLite after `last_sent_cursor`; notifications carry only cursor hints.
- Use bounded per-connection queues; close a durable-backpressured client with retryable reason and last safely processed semantics rather than dropping facts.
- Keep connection tasks owned by server shutdown and update replay/connect/disconnect/lag/slow-client metrics.

##### Data/state introduced

Ephemeral connection/subscription/send-queue/cursor state and `sync.complete`; no journal rows for socket traffic.

##### Contracts/interfaces introduced

At-least-once transport plus client dedup yields convergent projection; SQLite cursor, not broadcast sequence or wall time, is continuity.

##### Failure behavior

Socket/broadcast/send failure affects delivery only. Query/storage invariant failure closes connection and may degrade readiness; it never loses/changes committed work.

##### Validation

Commit between subscribe/high-water, notification drops/lag, reconnect from every cursor, internal cursor jumps, slow reader queue overflow, multiple clients, auth revoke on new connection, and server shutdown joins.

##### Exit criteria

- [ ] No committed public event is silently dropped.
- [ ] Replay-to-live transition has no race.
- [ ] Slow clients cannot block commits/work.
- [ ] WebSocket carries no command mutation.

##### What is deliberately NOT implemented yet

Assistant draft events, token/tool stdout streaming, WebSocket resume tokens, compression tuning, or client implementation.

## Stage 12: Workstation port, capabilities, logical paths, and real file reads

### Objective

Introduce the replaceable machine boundary and a partial `LocalWorkstation` that reports identity/capabilities and performs honest bounded file reads through logical workspace/path semantics.

### Why it happens now

The durable/runtime spine is stable. Machine access can now be built below tools without leaking filesystem APIs into application code.

### Preconditions

Workstation/workspace IDs/generation/config and artifact/error primitives exist; Ubuntu test access is available for target assertions.

### Exact implementation work

- Execute Substages 12.1–12.3.
- Advertise only capabilities actually implemented at this stage; production readiness remains false for shell/tool use.
- Enforce the rule that model-facing filesystem/process access occurs only through this port.

### Data/state introduced

Versioned capabilities snapshots, logical/resolved path evidence, file metadata/hash/content/error results, and workstation last-seen evidence.

### Contracts/interfaces introduced

Async `Workstation` operations: capabilities, read file, execute, inspect, cancel. Execute/inspect/cancel are typed now and implemented fully in Stage 13.

### Failure behavior

Identity/generation/workspace/path/read errors are normalized; partial/binary/oversized content never masquerades as a successful full text read.

### Validation

Port contract tests, relative/absolute/symlink/path edge cases, UTF-8/binary/special/directory/missing/permission/size cases, and boundary scans for direct model-facing filesystem use outside the adapter.

### Exit criteria

- [ ] Workstation identity/generation/capabilities are explicit.
- [ ] Logical and resolved paths are distinct.
- [ ] `read_file` matches all architecture semantics.
- [ ] Application/context/tool code has no direct filesystem/process API.

### What is deliberately NOT implemented yet

Shell execution, process handles, RemoteWorkstation, sandboxing, path confinement claims, file writes/edit tools, or high-level install/test methods.

### Substages

#### Substage 12.1: Define the Workstation port and canonical request/result types

##### Objective

Fix the smallest remote-capable machine contract before implementing local OS calls.

##### Why it happens now

Tools need stable machine primitives, and future remote extraction is costly if PIDs/paths leak now.

##### Preconditions

Domain IDs, errors, clocks, limits, workspace topology, and execution/artifact references exist.

##### Exact implementation work

- Define capabilities with workstation ID/generation/kind/OS/architecture/shell/read/foreground/cancel/inspect/privilege/cgroup/limits/workspaces.
- Define `FileReadRequest/Result`, `ExecutionRequest/Result`, `ExecutionInspection`, and `CancellationResult` using caller-generated operation/execution IDs.
- Represent path as workspace-relative or explicit absolute; represent cwd/environment/stdin/timeout/capture/resource/privilege policies explicitly.
- Keep stdout/stderr descriptors/results independent and local PID only in trace-only diagnostic fields.
- Make methods async/cancellation-aware and reject generation mismatch before OS action.

##### Data/state introduced

Canonical port values only.

##### Contracts/interfaces introduced

Workstation performs machine primitives; it owns no tools, authority, work, model, journal, client, or credential semantics.

##### Failure behavior

Unsupported capability, wrong generation, invalid request, or unknown execution returns normalized error without fallback behavior.

##### Validation

Serialization/trait contract tests with a fake local/remote implementation; ensure stable execution ID survives transport simulation and PID cannot be supplied as ID.

##### Exit criteria

- [ ] Interface contains only the five normative operations.
- [ ] Requests carry all execution context explicitly.
- [ ] No higher-level workflow method appears.

##### What is deliberately NOT implemented yet

RPC, SSH, remote credentials, daemons/sessions, package/Docker/service-specific methods, or tool schemas.

#### Substage 12.2: Implement capabilities, identity/generation checks, and path resolution

##### Objective

Bind configured logical workspaces to observed local paths and report truthful machine capabilities.

##### Why it happens now

File/process operations must share one resolver and generation guard rather than duplicate path logic.

##### Preconditions

Substage 12.1 and bootstrapped workstation/workspace mapping exist.

##### Exact implementation work

- Discover/validate Ubuntu release, CPU architecture, Bash path, cgroup v2/delegation, privilege modes, and configured bounds; persist/update capability evidence under controlled startup transaction.
- Resolve workspace ID to configured local root; normalize relative/absolute inputs, reject NUL/empty/overlong values, and record requested/logical/resolved forms.
- Canonicalize existing targets/symlinks for evidence while explicitly not claiming sandbox containment.
- Verify supplied workstation ID/generation equals active adapter identity before access.
- Provide a fake resolver/capability implementation for portable tests and real Ubuntu assertions for release.

##### Data/state introduced

Updated workstation capabilities/last-seen evidence and ephemeral resolved-path values.

##### Contracts/interfaces introduced

Absolute physical paths remain adapter evidence; model/application select workspace and logical path only through injected context.

##### Failure behavior

Missing/mismatched generation/workspace, invalid path, inaccessible root, or capability mismatch fails before I/O and may keep readiness false.

##### Validation

Workspace mapping/generation tests, symlink/path traversal normalization, nonexistent leaf handling, OS/arch/Bash/cgroup capability checks on Ubuntu, and no-path-as-identity assertions.

##### Exit criteria

- [ ] Capability report matches observed target behavior.
- [ ] One resolver is used by file and process operations.
- [ ] Generation mismatch prevents action.
- [ ] No security claim exceeds broad-authority V0.

##### What is deliberately NOT implemented yet

Workspace enrollment/replacement, remote path mapping, chroot/sandbox, or policy denial based solely on path.

#### Substage 12.3: Implement LocalWorkstation `read_file`

##### Objective

Perform the first real model-reachable Ubuntu observation with complete metadata and structured failures.

##### Why it happens now

Port/resolver/capabilities are fixed and no tool layer yet obscures low-level behavior.

##### Preconditions

Substages 12.1–12.2; configured workspace; file-read hard/default bounds.

##### Exact implementation work

- Validate request and resolve target under user privilege.
- Open regular files only; obtain type/size/optional modified metadata; reject directory/special files.
- Enforce requested/default 1 MiB and hard 8 MiB bounds without reading unbounded content; read bytes, validate UTF-8, compute SHA-256, and return exact content/metadata.
- For binary/non-UTF-8 return `binary_content` with safe size/hash evidence; for over-limit return `file_too_large` rather than an unmarked prefix.
- Keep physical path out of model/client unless Tool Execution Service creates the approved logical/safe projection.

##### Data/state introduced

Observed file result in memory; later tool service persists it. No workspace file is changed.

##### Contracts/interfaces introduced

Successful V0 file result is complete (`truncated=false`); failure never fabricates empty content.

##### Failure behavior

Not found, permission, binary, too large, invalid path, changed-during-read/I/O, generation mismatch, and deadline/cancellation remain distinct normalized outcomes.

##### Validation

Real filesystem fixtures for UTF-8, empty, multibyte, binary, exact limit, over limit, symlink, FIFO/device/directory, missing, permission, deadline, and concurrent modification; verify hashes/paths.

##### Exit criteria

- [ ] Every architecture read case has a test.
- [ ] Result claims exactly what was observed.
- [ ] No lossy decode is presented as source text.
- [ ] Only LocalWorkstation performs this model-facing read.

##### What is deliberately NOT implemented yet

Streaming file reads, directory listing, writes/patches, MIME detection services, or automatic artifact promotion.

## Stage 13: LocalWorkstation foreground process execution and privilege

### Objective

Implement real bounded Bash execution with explicit cwd, clean environment, user/admin privilege, owned process group/cgroup, concurrent capture, inspect/cancel, timeout escalation, descendant cleanup, and complete results.

### Why it happens now

The Workstation contract, resolver, artifact writer, cancellation seam, and Linux test requirement are established. Tool persistence comes next and must call a proven machine adapter.

### Preconditions

Stage 12 passes; an Ubuntu 24.04 systemd/cgroup-v2 test environment can delegate a subtree; sudo/admin policy can be tested safely.

### Exact implementation work

- Execute Substages 13.1–13.4.
- Keep all model-facing `Command`/process APIs inside `adapters/local_workstation`.
- Treat `kill_on_drop` as defense only; explicit termination/reap/verification owns cleanup.

### Data/state introduced

Ephemeral execution-handle registry, process group/cgroup identifiers, capture files/descriptors, timing/exit/signal/cleanup results, and trace-only PIDs.

### Contracts/interfaces introduced

`execute`, `inspect_execution`, and `cancel_execution` with stable execution ID, foreground-only semantics, and result certainty.

### Failure behavior

Spawn/timeout/cancel/signal/nonzero/cleanup are distinct. If process-tree cleanup cannot be confirmed, the adapter reports cleanup failure/uncertainty for Tool Service to classify as outcome unknown.

### Validation

Ubuntu integration tests cover shell/cwd/env/FDs, user/admin identity, high-output drains, exit/signal/spawn, TERM/KILL, child/grandchild/background cleanup, inspect/cancel races, and no secret inheritance.

### Exit criteria

- [ ] Real Bash commands execute only through LocalWorkstation.
- [ ] Every process tree is owned, reaped, and verified or reported uncertain.
- [ ] Capture is bounded without pipe deadlock.
- [ ] User/admin paths and environment sanitation are evidence-backed.

### What is deliberately NOT implemented yet

Durable terminals, daemons/process sessions, hostile-root containment, RemoteWorkstation, automatic command retry, or tool-facing shell schema.

### Substages

#### Substage 13.1: Implement Bash launch, cwd, environment, stdin, descriptors, and privilege

##### Objective

Construct the exact child process boundary without interpolating commands or inheriting backend secrets/state.

##### Why it happens now

Containment/capture can only be trusted after spawn semantics are fixed.

##### Preconditions

Execution request validation, path resolver, secret wrappers, and configured shell/env limits exist.

##### Exact implementation work

- Validate command UTF-8/nonempty/64 KiB max, timeout 120 default/900 hard max, cwd workspace-relative/absolute directory, and effective privilege already authorized above.
- Invoke `/bin/bash --noprofile --norc -o pipefail -c <command>` as direct arguments, non-login/noninteractive, fresh per call, stdin `/dev/null`, separate stdout/stderr pipes.
- Clear environment then set the exact user-mode allowlist and nonsecret work/workspace IDs. Close or mark close-on-exec all unrelated descriptors.
- For administrative mode use a reviewed `sudo -n` launcher with a clean root environment (`env -i` or equivalent), never `sudo -E`, never interpolate through a second shell, and record effective UID/privilege.
- Resolve/record starting cwd before spawn; do not persist shell variables or cwd across calls.

##### Data/state introduced

One ephemeral child launch specification and trace-safe command hash/summary; no durable record yet.

##### Contracts/interfaces introduced

Command text is arbitrary code on the dev workstation but not a credential/authority carrier; effective privilege is injected, not trusted from model fields.

##### Failure behavior

Validation/cwd/sudo/spawn errors return before or at start with `start_observed=false`; exit 127 after Bash starts is not spawn failure.

##### Validation

Quoting/metacharacter/pipes/redirection tests, cwd/default/fresh shell tests, sentinel backend secret/env/FD scan, stdin EOF, UID/GID/user/admin checks, and no profile loading.

##### Exit criteria

- [ ] Exact Bash invocation is tested.
- [ ] Child environment contains only allowlisted values.
- [ ] Admin mode is noninteractive/clean and explicit.
- [ ] Command is never re-shell-interpolated.

##### What is deliberately NOT implemented yet

Per-command credentials, interactive stdin/PTY, persistent shell, command sanitization, or production authority.

#### Substage 13.2: Implement execution registry, process groups/cgroups, inspection, and cancellation

##### Objective

Own each live foreground command by stable execution ID and stop its descendants reliably.

##### Why it happens now

Launch semantics exist; output and result logic require a lifecycle handle with deterministic cleanup.

##### Preconditions

Substage 13.1 and delegated cgroup v2 environment are available.

##### Exact implementation work

- Create a new Unix session/process group and per-execution cgroup within the service's delegated subtree before/at spawn; move child into it without racing unowned descendants as far as Linux permits.
- Store an ephemeral execution-ID-to-handle map containing runtime-local child/group/cgroup/cancellation state; reject duplicate live execution IDs.
- Implement inspection for known current-runtime handles, reporting live/terminal observations only; absence after restart is `inspection_not_found`, not proof of nonexecution.
- Implement cancel/timeout: TERM group/cgroup, wait configured 5 seconds, KILL remaining members, reap direct child, close I/O, verify cgroup empty, remove safe subtree.
- At ordinary child exit, terminate any surviving descendants before reporting cleanup complete. Add `kill_on_drop` only as secondary protection.

##### Data/state introduced

Ephemeral handles, group/cgroup IDs, cancellation phase/timestamps, and cleanup evidence.

##### Contracts/interfaces introduced

Stable execution ID—not PID—is the operation identity; current-runtime inspection has deliberately limited knowledge.

##### Failure behavior

Failure to signal/reap/verify emptiness/remove cgroup reports `cleanup_failed`; it must not be converted to a clean timeout/cancel result.

##### Validation

Direct child, child/grandchild, shell backgrounding, TERM-ignore, fast-exit, PID reuse, duplicate ID, inspect/cancel races, and cgroup-empty assertions on Ubuntu.

##### Exit criteria

- [ ] Direct and ordinary descendant processes cannot survive reported completion.
- [ ] Inspection never overclaims after restart.
- [ ] Cancellation escalation/reaping is deterministic.
- [ ] Cleanup uncertainty remains explicit.

##### What is deliberately NOT implemented yet

Adversarial root escape prevention, durable handles, OS-service tracking, cross-runtime inspection, or remote cancellation.

#### Substage 13.3: Implement concurrent bounded stdout/stderr capture and execution results

##### Objective

Drain both pipes without deadlock, retain bounded raw evidence, and return honest counts/projections/results.

##### Why it happens now

The process lifecycle is owned; capture must join before terminal cleanup/result can be observed.

##### Preconditions

Artifact streaming/finalization and output policy exist.

##### Exact implementation work

- Spawn owned drain tasks immediately for stdout/stderr; stream independently into artifact writers up to 8 MiB each while continuing to read/discard/count beyond cap.
- Use saturating 64-bit observed counts and exact captured counts; generate 24 KiB head + 8 KiB tail per-stream model projection when needed and mark invalid UTF-8 replacements.
- Wait for child and both drains, perform descendant cleanup, finalize artifact descriptors, and construct `ExecutionResult` with start/cwd/privilege/result kind/exit/signal/timeout/cancel/duration/counts/truncation/cleanup/error.
- Never claim merged stdout/stderr ordering.
- Remove live handle only after terminal result/cleanup is assembled; leave persistence to Tool Service.

##### Data/state introduced

Finalized unreferenced capture artifacts and in-memory execution result; capture/process timings.

##### Contracts/interfaces introduced

Process exit observation, output drain completion, and tree cleanup all precede a definitive result.

##### Failure behavior

Drain/artifact failure triggers cleanup and returns artifact/internal error; if side effects ran but durable upper-layer outcome cannot be committed, Tool Service later marks unknown.

##### Validation

Interleaved high-volume stdout/stderr, exact/over cap, binary bytes, closed pipe, slow reader/writer, signal/timeout, artifact hashes/head-tail/counts, and no deadlock/resource leak.

##### Exit criteria

- [ ] Both streams drain concurrently through cap and beyond.
- [ ] Observed/captured/inline/omitted counts reconcile.
- [ ] Definitive result waits for cleanup/drains.
- [ ] Raw output is absent from tracing.

##### What is deliberately NOT implemented yet

Live stdout WebSocket streaming, unlimited capture, exact cross-stream ordering, search indexing, or remote artifact backend.

#### Substage 13.4: Prove Ubuntu/admin compatibility and expose process crash markers

##### Objective

Validate real target semantics and make spawn/exit/cleanup boundaries available to later durable crash tests.

##### Why it happens now

LocalWorkstation must be proven independently before Tool Service surrounds it with intent/outcome records.

##### Preconditions

Substages 13.1–13.3 run under an Ubuntu 24.04 systemd unit with `Delegate=yes` and safe sudo policy.

##### Exact implementation work

- Build a target test service/unit/user/workspace with cgroup delegation and noninteractive sudo; do not grant production credentials.
- Test ordinary Git/compiler/package manager commands plus safe disposable administrative package-query/install/remove, Docker info/run/cleanup, and temporary systemd service operations.
- Confirm backend remains non-root and admin child uses clean root identity; document Docker/root equivalence honestly.
- Add test markers immediately after process spawn, after direct child exit before upper-layer outcome, during TERM/KILL, and after cleanup verification.
- Run repeated leak scans for PIDs/cgroup members/file descriptors/temp artifacts after pass/fail/timeout/cancel.

##### Data/state introduced

Disposable Ubuntu test packages/containers/services/workspace files plus test evidence; no product records.

##### Contracts/interfaces introduced

Target capability flags are enabled only after corresponding tests pass; process markers identify observed adapter phases, not durable tool state.

##### Failure behavior

Unavailable cgroup delegation/sudo/Docker capability disables readiness for advertised capability and fails target gate; it is not papered over with weaker cleanup.

##### Validation

Run the full workstation suite on exact Ubuntu/systemd conditions, including reboot/service restart and security-sentinel scans.

##### Exit criteria

- [ ] User and administrative engineering workflows work on Ubuntu.
- [ ] Cgroup/process cleanup holds under repeated stress.
- [ ] Spawn/exit/cleanup markers are deterministic.
- [ ] Capability report matches verified host behavior.

##### What is deliberately NOT implemented yet

EC2 provisioning, durable tool intent, malicious root containment, production systems, or long-lived service supervision by Craxii.

## Stage 14: Tool Registry, authority seam, and Tool Execution Service

### Objective

Expose only `read_file` and `run_shell` to models through immutable definitions, validate and authorize calls, persist requested/dispatch/outcome ordering, invoke LocalWorkstation, and return structured bounded results without hidden retry.

### Why it happens now

Journal/evidence transactions, artifact store, cancellation, and LocalWorkstation are proven. The model system can now rely on a complete tool boundary.

### Preconditions

Stages 8, 10, 12, and 13 pass; active work/completed source invocation fixtures can be created.

### Exact implementation work

- Execute Substages 14.1–14.4.
- Keep Tool Execution Service above registry/handlers/Workstation and as sole owner of tool journal transitions.
- Activate every tool/process/artifact crash window with durable assertions.

### Data/state introduced

Tool definitions/fingerprint, authority decisions, requested/dispatching/completed/interrupted/unknown execution rows/events, output artifacts, work waiting/resumed/interrupted transitions, and tool measurements.

### Contracts/interfaces introduced

Immutable `ToolRegistry`, typed handlers, V0 `AuthorityEvaluator`, and `ToolExecutionService.execute_call` returning canonical model result only after terminal persistence.

### Failure behavior

Unknown/invalid/denied/observed OS results are structured completed outcomes where definite. Crash/cleanup ambiguity after dispatch is `outcome_unknown` and interrupts work. No infrastructure retry executes a tool twice.

### Validation

Schema/decoder equivalence, duplicate registration/call IDs, all tool results, privilege/cancel races, intent order, artifact linkage, failpoint side-effect marker, and journal/projection tests.

### Exit criteria

- [ ] Registry exposes exactly two V0 tools in stable order.
- [ ] Every Workstation call has durable dispatch intent first.
- [ ] Every model-visible result has committed evidence.
- [ ] Unknown side effects stop work and never repeat.

### What is deliberately NOT implemented yet

More tools, MCP, browser/cloud/database APIs, parallel calls, credential injection, production authority, or agent loop integration.

### Substages

#### Substage 14.1: Implement tool definitions, schemas, handlers, and immutable registry

##### Objective

Make tool identity/schema/behavior deterministic and provider-independent.

##### Why it happens now

Tool Service and future model context require one trusted definition snapshot.

##### Preconditions

Typed read/shell inputs/results and approved schema-generation dependency exist.

##### Exact implementation work

- Define stable names, semantic implementation version, schema version, concise description, JSON Schema, typed Serde decoder denying unknown fields, canonical result type, default/hard timeout, output policy, capabilities, side-effect possibility, privilege modes, and handler.
- Implement registry startup registration, duplicate rejection, stable ordering, exact lookup, capability aggregation, and deterministic toolset fingerprint.
- Derive schema and decoder from one source or golden-test equivalence; test provider-independent schema constraints.
- Implement thin typed `read_file` and `run_shell` handlers that use injected Workstation/context only and never write journal or retry.
- Register exactly these two tools in production composition.

##### Data/state introduced

Immutable in-memory registry/definitions/fingerprint; no durable state until context/attempt snapshots reference it.

##### Contracts/interfaces introduced

Registry resolves trusted handlers; model supplies ordinary arguments only; handlers receive injected workspace/deadline/output/authority context.

##### Failure behavior

Duplicate/invalid definition fails startup. Unknown name returns structured `unknown_tool`, never shell fallback.

##### Validation

Definition snapshots/fingerprint stability, schema-versus-decoder corpus/property tests, duplicate/order/lookup/capability cases, and handler boundary mocks.

##### Exit criteria

- [ ] Registry is immutable after startup.
- [ ] Schemas and decoders cannot drift unnoticed.
- [ ] Handlers have no State Store/provider access.
- [ ] Only two production definitions exist.

##### What is deliberately NOT implemented yet

Dynamic plugins, version negotiation, tool discovery service, high-level workflows, or third-party tool sources.

#### Substage 14.2: Implement the V0 authority evaluator and requested transaction

##### Objective

Validate source/work/call/arguments and persist the model request plus an auditable local authority decision before dispatch.

##### Why it happens now

Registry is trusted and attempt schema is available; dispatch must never infer authority from model-hidden fields.

##### Preconditions

Completed source model invocation fixture, active work ownership, and registry exist.

##### Exact implementation work

- Verify completed source invocation belongs to active current work; provider tool-call ID is unique within it; call order/limits are valid.
- Resolve tool and parse typed args. For unknown tools use a reserved unresolved definition-version marker solely for evidence; for known-tool schema failure retain known versions. Persist a structured completed validation result without Workstation dispatch.
- Generate tool execution and stable execution IDs; inject Craxii/work/workstation generation/workspace/cwd/deadline/output policy and requested privilege.
- Commit `tool.execution_requested`, complete arguments/hash/context, and work `waiting_on_tool` atomically.
- Evaluate typed V0 development-workstation policy (`allow|deny`, effective privilege, policy version/reason) and record its snapshot. Denial becomes a definite structured completed result without dispatch.

##### Data/state introduced

Requested or immediately completed validation/authority-denied execution rows/events and work waiting/resumed transitions.

##### Contracts/interfaces introduced

Model never chooses workspace/generation/effective privilege/credential scope. Policy is simple but its input/output seam is durable.

##### Failure behavior

Stale work/source/duplicate call/cancel is conflict; malformed/unknown/denied is an observed tool result; internal persistence failure returns no model result.

##### Validation

Source/work/call ownership, duplicate ID, every argument edge, unknown tool, deny/allow/user/admin decision, cancellation race, and exact event/order tests.

##### Exit criteria

- [ ] Every complete model call is either rejected structurally or has one execution identity.
- [ ] Authority-bearing context is injected above the handler.
- [ ] Validation/denial never reaches Workstation.
- [ ] Requested intent is durable before any possible dispatch.

##### What is deliberately NOT implemented yet

External Authority Service, natural-language policy compilation, user approval, credentials, or project isolation.

#### Substage 14.3: Implement dispatch-intent, Workstation invocation, artifact finalization, and outcome transaction

##### Objective

Surround the real machine action with durable dispatch intent and durable observed terminal evidence.

##### Why it happens now

Requested records/policy and proven LocalWorkstation exist; this is the architecture's critical side-effect boundary.

##### Preconditions

Substage 14.2 allowed decision and noncancelled active work.

##### Exact implementation work

- Recheck durable/in-memory cancellation and expected work/runtime state immediately before dispatch.
- Atomically set tool `dispatching`, record effective privilege/deadline/output policy/resolved request evidence, append `tool.execution_dispatching`, and commit.
- Invoke the typed handler/Workstation outside any DB transaction; pass stable execution ID and cancellation handle exactly once.
- Finalize capture/read evidence artifacts, construct bounded canonical model result, then atomically persist observed result/metrics/artifact references, append `tool.execution_completed`, append `work.resumed`, clear current tool, and return work to running.
- For unconfirmed cleanup/lost runtime/store failure after side effect, use recovery/outcome-unknown path and never expose an ordinary result.

##### Data/state introduced

Dispatch and terminal execution evidence, artifacts, tool/work events, timing/counts/privilege/cleanup/error fields.

##### Contracts/interfaces introduced

No transaction spans machine access; a result is eligible for context only after the outcome transaction commits.

##### Failure behavior

Observed nonzero/not-found/timeout/cancel with confirmed cleanup is definite completed evidence. Any ambiguity after dispatch interrupts work and prohibits repeat.

##### Validation

Successful read/shell, every observed error/result kind, artifact failure, cancellation at every checkpoint, stale state, and exact journal offset order versus process marker.

##### Exit criteria

- [ ] Dispatch event commits before spawn/read marker.
- [ ] Outcome event commits after observation/artifact finalization.
- [ ] Model never sees uncommitted result.
- [ ] No code path calls Workstation twice for one execution ID.

##### What is deliberately NOT implemented yet

Tool rollback, automatic retry, concurrent tool execution, remote dedup/reconciliation, or provider continuation.

#### Substage 14.4: Complete structured tool tests and all tool-side crash windows

##### Objective

Prove tool correctness and honest ambiguity across every required persistence/process/artifact boundary.

##### Why it happens now

The complete side-effect path exists and must be crash-safe before a model can drive it.

##### Preconditions

Substages 14.1–14.3 and failpoint/process marker infrastructure pass.

##### Exact implementation work

- Activate `after_tool_requested_commit`, `after_tool_dispatch_intent_commit`, `after_tool_process_spawn`, `after_tool_process_exit_before_outcome_commit`, and reuse `after_artifact_rename_before_db_commit`.
- Use a disposable marker command that appends/fsyncs execution ID then sleeps; kill at requested, dispatch, spawn, exit, and artifact windows.
- Recover with a new runtime and assert requested-before-dispatch becomes `interrupted_before_dispatch`; any dispatching-without-terminal becomes `outcome_unknown`; owning work becomes interrupted; no automatic handler/execute repeat occurs.
- Verify zero-or-one marker occurrence but never a second; cgroup emptiness after service cleanup; exact event/state/artifact/orphan counts; replay candidate visibility.
- Run comprehensive real `read_file`/`run_shell` failure/output/privilege/cancel tests and scan for direct process/filesystem usage outside allowed adapters/infrastructure.

##### Data/state introduced

Crash fixture databases/artifacts/workspaces and recovery events; no new production schema.

##### Contracts/interfaces introduced

The side-effect marker suite is the release oracle for honest at-most-one automatic dispatch, not exactly-once external effects.

##### Failure behavior

An unexplained marker count, leaked descendant, repeated execution ID, false terminal result, or missing recovery event fails the stage.

##### Validation

Repeated deterministic crash matrix on Ubuntu, projector/integrity checks after each, and release binary failpoint absence.

##### Exit criteria

- [ ] All five tool crash windows pass.
- [ ] Observed failures remain structured and loop-safe.
- [ ] Ambiguous effects remain unknown/interrupted and unrepeated.
- [ ] Process/artifact cleanup evidence is complete.

##### What is deliberately NOT implemented yet

Random destructive chaos, production credentials/systems, long-lived services as executions, or model-driven acceptance work.

## Stage 15: Canonical model system and deterministic scripted provider

### Objective

Define provider-independent model targets/capabilities/requests/ordered responses/usage/stream events, deterministic selection and limits, a provider port, and a fully scripted test provider.

### Why it happens now

Tool definitions/results exist, so canonical model types can represent the real loop without importing OpenAI. Context and invocation orchestration depend on these contracts.

### Preconditions

Stages 3–4 and 14 pass; typed model target configuration exists.

### Exact implementation work

- Execute Substages 15.1–15.4.
- Keep V0 selection deterministic and target data static at startup.
- Make scripted streams capable of every ordered/failure behavior needed by later crash tests.

### Data/state introduced

Configured model-target/capability snapshots, canonical request/response/usage/stream values, selection results/reasons, token estimates, scripted fixture programs, and limit/retry classifications.

### Contracts/interfaces introduced

`ModelSelectionPolicy`, `ModelProvider`, provider stream, token estimator, typed native options, ordered output items, and scripted adapter.

### Failure behavior

No capable/default target, invalid config, malformed/unknown semantic output, limit breach, or scripted failure returns normalized model/selection/provider errors. No tool action follows partial calls.

### Validation

Target/selection matrices, canonical serialization, ordered mixed-output/terminality, estimator/limit, retry classifier, and scripted provider scenarios.

### Exit criteria

- [ ] No OpenAI type exists outside its future adapter.
- [ ] Target selection precedes model-specific context rendering.
- [ ] Ordered response supports mixed text/tools/refusal/opaque data.
- [ ] Scripted provider can deterministically drive the full future loop.

### What is deliberately NOT implemented yet

Context queries/manifests, invocation persistence, agent loop, real OpenAI HTTP, multiple real providers, dynamic discovery, pricing/routing optimization, or provider-owned conversation state.

### Substages

#### Substage 15.1: Implement targets, capabilities, required capabilities, and deterministic selection

##### Objective

Choose one capable configured target and record why without rendering provider input first.

##### Why it happens now

Context limits/format depend on the target, and the architecture explicitly forbids the reverse ordering.

##### Preconditions

Validated model config and toolset capability requirements exist.

##### Exact implementation work

- Define target ID/provider/model/config version/enabled flag, the exact canonical capability set, context/output limits, typed provider-native options, estimator ID, and optional observability classes.
- Derive required capabilities from text/toolset/current work and any explicit configured target request.
- Select explicit capable enabled target or the configured default; record considered IDs, requirements, `explicit|configured_default`, config version, and timestamp.
- Fail if incapable/unavailable; never remove tools or choose undeclared fallback.
- Validate startup has one usable default for production readiness later.

##### Data/state introduced

Immutable startup registry/config snapshot and per-invocation selection result in memory; later invocation rows persist it.

##### Contracts/interfaces introduced

Selection uses cheap requirements/stats, then Context Assembler renders for the selected target.

##### Failure behavior

Missing/duplicate/disabled/incapable target is `model_selection_error`; no fallback ladder or provider call occurs.

##### Validation

Explicit/default/disabled/missing/capability mismatch/tool requirement/config version cases and deterministic considered-order fixtures.

##### Exit criteria

- [ ] One target is chosen deterministically or failure is explicit.
- [ ] No task classifier/cost router exists.
- [ ] Limits/options are versioned data, not scattered constants.

##### What is deliberately NOT implemented yet

Learned routing, availability failover, automatic model substitution, live model discovery, or pricing decisions.

#### Substage 15.2: Define canonical request, ordered response, usage, continuation, and stream types

##### Objective

Represent all V0 inference semantics without collapsing order or provider-specific meaning.

##### Why it happens now

Context and agent loop need stable input/output shapes before persistence/wire mapping.

##### Preconditions

Target/capability types and tool definitions/results exist.

##### Exact implementation work

- Define ordered input variants for system/developer/user/assistant content, canonical tool call/result pairing, refusal/structured data, synthetic status, and provider-guarded opaque continuation.
- Define request with logical invocation, target, instructions/input/tools/output limit/tool choice, `parallel_tool_calls=false`, typed native options, and manifest ID.
- Define response with ordered text parts/tool calls/structured/refusal/reasoning summary/provider opaque/unknown item, stop/incomplete reason, usage breakdown, provider IDs/metadata/continuation.
- Define internal stream events for lifecycle, safe text/refusal deltas, bounded tool-argument deltas/completion, item completion, terminal response/error/usage.
- Define canonical hashes/redacted artifact representations; raw provider JSON is diagnostic only.

##### Data/state introduced

Canonical model values and fixture encodings only.

##### Contracts/interfaces introduced

Provider preserves order; complete containing response and arguments are required before tool eligibility; runtime decides terminality.

##### Failure behavior

Duplicate call ID, invalid UTF-8/JSON, oversized arguments/items, unknown correctness-bearing item, empty terminal output, or invalid pairing fails closed.

##### Validation

Golden mixed-order fixtures, multiple calls, text with tools, refusal, structured data, opaque same/different provider, unknown item, partial args, and terminal-decision tests.

##### Exit criteria

- [ ] Text and tools can coexist in order.
- [ ] Tool result pairs to canonical call ID.
- [ ] Native continuation is optional evidence, never history truth.
- [ ] Unknown semantic items cannot be ignored silently.

##### What is deliberately NOT implemented yet

Provider wire structs, built-in provider tools, parallel dispatch, multimodal client content, or global interpretation of opaque reasoning.

#### Substage 15.3: Define provider/estimator ports, retry classification, and hard limits

##### Objective

Separate provider transport/normalization from application-owned retries, cancellation, and loop limits.

##### Why it happens now

The scripted provider and Model Gateway need exact responsibilities and boundaries.

##### Preconditions

Substages 15.1–15.2 and normalized errors/clocks/cancellation exist.

##### Exact implementation work

- Define `capabilities`, conservative token estimate, streaming invoke, complete normalization, and provider error classification on the port/adapter boundary.
- Define retry policy inputs/results: maximum initial plus two retries, pre-semantic-output transient classes, jitter/backoff/Retry-After caps, cancellation, and billing ambiguity evidence.
- Encode hard/default model/work/loop/tool/output/argument/invocation/idle limits from architecture as validated runtime config.
- Define fallback estimator requirements: documented conservative upper bound and estimator ID/version; compare later with provider usage.
- Make retry attempt identity distinct while logical invocation/context manifest remains shared.

##### Data/state introduced

Port command/result values, retry decisions/delays, estimator results, and limit policy snapshot.

##### Contracts/interfaces introduced

Adapter classifies external condition; Model Gateway decides bounded retry; retry cannot duplicate Workstation side effects because calls are persisted/processed only after complete response.

##### Failure behavior

Auth/permission/invalid/context/safety/malformed-after-output/unknown semantic/cancel are nonretry; transient pre-output may retry boundedly; exhausted becomes definite provider failure.

##### Validation

Complete retry decision table, deterministic jitter clock/RNG tests, Retry-After cap, exposed-draft prohibition, cancellation during backoff, and every hard limit boundary.

##### Exit criteria

- [ ] Retry policy is explicit and application-owned.
- [ ] Attempts have unique IDs and bounded count.
- [ ] Token estimation identifies method/version.
- [ ] No limit can silently drop content/calls.

##### What is deliberately NOT implemented yet

Tool retries, provider request idempotency assumptions, adaptive backoff, circuit breakers, or cost budgets.

#### Substage 15.4: Implement the ScriptedProvider and provider contract suite

##### Objective

Provide deterministic streaming/inference behavior for context, agent-loop, crash, and protocol tests without API nondeterminism.

##### Why it happens now

Canonical provider contracts and limits are fixed; the next stages can build the hardest orchestration against a controllable edge.

##### Preconditions

Substages 15.1–15.3 and test clocks/failpoint controller exist.

##### Exact implementation work

- Implement scripts keyed by expected invocation/context hash and prior tool result, producing timed stream events and one complete canonical response/error.
- Cover final text; text then tool; multiple tools; refusal; structured output; malformed/partial/oversized arguments; duplicate call IDs; transient pre-output failure then success; failure after draft; idle/overall timeout; cancellation; unknown provider item; opaque continuation; canonical machine-inspection answer.
- Record calls/attempts/cancellation so tests can assert exact invocation counts and no hidden retries.
- Add reusable redacted fixtures and mismatch diagnostics that show IDs/hashes, not raw sensitive content by default.

##### Data/state introduced

Test-only scripted programs, captured canonical requests, stream markers, and deterministic usage/latency values.

##### Contracts/interfaces introduced

ScriptedProvider implements exactly the same port as OpenAI; no test-only shortcut may bypass Model Gateway/State Store later.

##### Failure behavior

Unexpected call/order/hash fails test immediately. Cancellation closes the scripted stream and records which semantic output escaped.

##### Validation

Run every script through the provider contract suite, including bounded accumulator/argument behavior, cancellation, retry classification, and ordered normalization.

##### Exit criteria

- [ ] All required model scenarios are deterministic.
- [ ] Exact request/attempt/cancel counts are observable.
- [ ] The provider boundary is testable without OpenAI.
- [ ] No application test imports future OpenAI wire fixtures directly.

##### What is deliberately NOT implemented yet

Model quality simulation, stochastic evaluation, live API, provider SDK, or second-provider proof.

## Stage 16: Causal Context Assembler and exact manifest construction

### Objective

Assemble complete causally eligible full history for a selected target, exclude later queued input, preserve order/provenance, enforce token limits without compaction, and produce an exact manifest ready for atomic invocation persistence.

### Why it happens now

Work/input/journal/tool evidence and target/estimator contracts exist. The agent loop cannot safely invoke a model until context truth is deterministic.

### Preconditions

Stages 7–8, 14, and 15 pass; selected target and active-work fixtures exist.

### Exact implementation work

- Execute Substages 16.1–16.4.
- Keep provider wire formatting out of the assembler.
- Do not persist a successful invocation manifest by itself; Stage 17 commits it atomically with model intent. Context-limit failure evidence uses its own explicit work-failure transaction.

### Data/state introduced

Read-snapshot eligibility cutoffs, ordered canonical source candidates/items, prepared manifests/source rows, package/request hashes, byte/token contributions, omissions/transforms, and context statistics.

### Contracts/interfaces introduced

`ContextAssembler.assemble(work_id, selected_target, policy/prompt/toolset versions)` returning `ContextPackage` plus `PreparedContextManifest` or explicit `context_limit_exceeded` evidence.

### Failure behavior

Missing/corrupt/unverifiable source is a context/invariant failure. If all eligible history plus reserve does not fit, fail explicitly; never compact, summarize, reorder, omit silently, or ask the provider to truncate.

### Validation

Golden history/order/manifests, later-message interleavings, interrupted/unknown synthetic status, tool loops, content hashes/bytes, estimator bounds, limit edges, snapshot isolation, and reproducibility after reopen.

### Exit criteria

- [ ] Eligibility is work/input/ordinal based, not latest-conversation based.
- [ ] Every rendered item has an exact source/transform/hash/position.
- [ ] Selected target limits control rendering and estimation.
- [ ] No compaction or unreported omission exists.

### What is deliberately NOT implemented yet

Invocation transactions/provider calls, memory, retrieval, compaction, summarization, vector/full-text search, model auto-truncation, or queued-message steering.

### Substages

#### Substage 16.1: Implement causal eligibility snapshots and cutoff queries

##### Objective

Select exactly the history that may influence active work `N` under one SQLite read snapshot.

##### Why it happens now

Context correctness begins with selection; later rendering cannot repair leaked input.

##### Preconditions

Work ordinals, trigger inputs, journal links, detailed model/tool evidence, and snapshot read infrastructure exist.

##### Exact implementation work

- Load active work/Craxii/conversation/workspace and verify expected runtime/state/version.
- Within one bounded read transaction select prior work ordinals `<N`, their committed user/assistant messages and observed eligible outputs; select exactly active work trigger event/input and completed current-work invocation/tool results by step/ordinal.
- Exclude ordinals `>N` regardless of their journal offset, all drafts/partial args/unobserved outcomes/secrets/traces/UI content/provider-only history.
- Capture conversation/work/ordinal, highest prior terminal ordinal, exact input event IDs, active output record IDs, and maximum journal offset observed as `EligibilityCutoff`.
- Load earlier failed/interrupted/unknown facts needed for synthetic status without converting uncertainty.

##### Data/state introduced

Ephemeral immutable eligibility snapshot/source references and cutoff.

##### Contracts/interfaces introduced

Eligibility is a relation/query over durable provenance; journal high-water is evidence/tie-breaker, not the sole cutoff.

##### Failure behavior

Missing trigger, nonmatching correlation, incomplete referenced artifact/output, stale runtime ownership, or duplicate logical position fails assembly; queued rows are never “best effort” filtered after rendering.

##### Validation

Commit later message before/during/after active events, reorder journal offsets deliberately, attach tool loops, interrupted prior work, and assert exact included/excluded IDs under concurrent writes.

##### Exit criteria

- [ ] A queued N+1 message is absent from every N snapshot.
- [ ] Active trigger appears once.
- [ ] Only observed committed outputs are eligible.
- [ ] Cutoff records enough facts to reproduce selection.

##### What is deliberately NOT implemented yet

Steering relationships, background triggers, semantic relevance ranking, history deletion, or provider-side history fetch.

#### Substage 16.2: Implement canonical source ordering and context package rendering

##### Objective

Transform eligible durable sources into one provider-independent ordered briefing without duplicating content.

##### Why it happens now

The exact candidate set is stable; canonical ordering and tool/result pairing can now be tested independently of token limits.

##### Preconditions

Substage 16.1; versioned system/developer prompt, workstation capability summary, workspace logical identity, and registry fingerprint/definitions exist.

##### Exact implementation work

- Render versioned instructions, safe workstation/workspace capability summary, and stable ordered tool definitions.
- Order prior work by conversation ordinal; within work by causal content/agent step/tool ordinal and journal offset tie-break; then active trigger; then active-work model/tool trace.
- Render user/assistant content exactly once, ordered model output including mixed text/tool calls, paired observed tool results, refusals/structured data when relevant, and provider-eligible opaque continuation guarded by provider identity.
- Generate explicit synthetic items for earlier failed-without-message, interrupted, outcome-unknown, and abandoned-draft facts using durable source IDs.
- Apply only documented representation transforms/bounded tool-result projection; preserve source hash/bytes and never expose artifact paths/secrets/raw traces.

##### Data/state introduced

Ordered canonical input/instruction/tool items and per-source transformation records.

##### Contracts/interfaces introduced

Provider adapters may translate roles/items but cannot alter causal order or pairings; the current user message is not duplicated as history plus prompt.

##### Failure behavior

Unpairable tool result/call, provider-mismatched opaque item, unknown correctness-bearing output, or unsafe secret source fails closed.

##### Validation

Golden multi-work/multi-step/multiple-tool/mixed-text/refusal/interruption fixtures; duplicate-content detection; cross-provider opaque exclusion; deterministic toolset/prompt hashes.

##### Exit criteria

- [ ] Canonical ordering exactly matches architecture.
- [ ] Every tool result follows its call and preserves order.
- [ ] Synthetic uncertainty is explicit and sourced.
- [ ] No provider wire role/event enters output.

##### What is deliberately NOT implemented yet

Prompt experimentation service, summarization, dynamic system instructions, semantic memory, or provider conversation references as history.

#### Substage 16.3: Implement token budgeting and explicit context-limit failure

##### Objective

Prove full eligible context plus reserved output fits the selected target before provider dispatch.

##### Why it happens now

Canonical items and target estimator/limits exist; this prevents provider truncation from becoming accidental context policy.

##### Preconditions

Substages 16.1–16.2 and selected-target estimator contract pass.

##### Exact implementation work

- Compute canonical bytes and rendered-request estimate including instructions, item framing, tool schemas, and provider/native overhead through the target estimator.
- Enforce `estimated_input_tokens + reserved_output_tokens <= context_window_tokens`, selected maximum output, and hard request/item/argument byte limits.
- Calculate contribution by system/tools/prior/current/source kind, utilization ratio, largest source, prior versus active share, and estimator identity/version.
- On overflow produce normalized `context_limit_exceeded` plus complete candidate/estimate/limit evidence; do not invoke provider or set provider truncation to auto.
- Make conservative fallback overestimate documented structure; never knowingly underestimate.

##### Data/state introduced

Token/byte/contribution estimates and either a fitting package or failed-assembly evidence.

##### Contracts/interfaces introduced

Context policy is all eligible V0 history or explicit failure; output reserve is not reclaimed silently.

##### Failure behavior

Estimator unavailable/overflow/unsafe underestimate is context error. Limit failure is definite work failure later, not a provider retry.

##### Validation

Exact below/equal/above boundaries, deliberately tiny target, huge tool schemas/results/messages, conservative fallback, overflow arithmetic, and provider-usage comparison fixtures.

##### Exit criteria

- [ ] Fitting decision is deterministic and versioned.
- [ ] Context-limit path has no provider call or hidden omission.
- [ ] Contribution statistics reconcile to totals.

##### What is deliberately NOT implemented yet

Automatic trimming, compaction, summarization, target fallback, adaptive reserve, or online tokenizer service.

#### Substage 16.4: Build exact manifest/provenance records and reconstruction tests

##### Objective

Produce the row/artifact material that can explain exactly what one invocation will see.

##### Why it happens now

Eligibility, rendering, and limits are final; invocation persistence next needs an immutable prepared manifest.

##### Preconditions

Substages 16.1–16.3 and context/evidence row codecs exist.

##### Exact implementation work

- Assign context manifest/logical invocation IDs and populate target/config/assembler/policy/prompt/toolset/cutoff/count/bytes/tokens/limits/utilization/omissions/hashes/timestamp.
- Create ordered source rows with exactly one source identity, role/kind/hash/rendered bytes/transform.
- Produce a provider-independent canonical request hash and optional redacted rendered-request artifact descriptor finalized before Stage 17's transaction.
- Implement manifest verifier: reload every source, recompute hash/order/transform/package/estimates where deterministic, and report drift.
- For context-limit failure, prepare an explicit evidence snapshot and work-failure payload; do not insert a fake provider invocation.

##### Data/state introduced

Prepared (not independently committed) manifest/source rows and optional finalized request artifact.

##### Contracts/interfaces introduced

A successful manifest commits only with its first model invocation intent; source manifest is provenance, not duplicate canonical history.

##### Failure behavior

Source hash drift, missing artifact, nondeterministic render, or row/source count mismatch prevents provider intent. Finalized unused artifact becomes an orphan candidate.

##### Validation

Round-trip/recompute after DB close/reopen, deterministic hashes across map ordering, exact source positions/counts/byte sums, and tampered source/artifact detection.

##### Exit criteria

- [ ] Every canonical package is reproducible from manifest sources.
- [ ] Prepared data is ready for one atomic attempt transaction.
- [ ] Limit failures retain enough evidence without fake invocation.

##### What is deliberately NOT implemented yet

Independent manifest commit for successful calls, provider request transmission, request-body logging, or long-term memory provenance.

## Stage 17: Model Gateway, explicit agent loop, and scheduler integration

### Objective

Persist and invoke bounded model attempts, process complete ordered output, execute tools sequentially, commit final assistant/refusal outcomes atomically, honor cancellation, and wire the real work runner into the scheduler.

### Why it happens now

Commands/scheduler/recovery, context, scripted provider, and tool service are all independently proven. This is the first safe point to assemble Craxii's explicit runtime loop.

### Preconditions

Stages 10 and 14–16 pass; production composition still uses no real provider.

### Exact implementation work

- Execute Substages 17.1–17.4.
- Preserve manifest+first intent atomicity and all intent/action/outcome ordering.
- Make cancellation and limit checks visible at every architecture checkpoint.

### Data/state introduced

Committed context manifests/sources, model attempts/events/usage/output, work waiting/resumed/terminal state, tool continuations, assistant messages/events, draft lifecycle inputs, and scheduler-owned agent-loop tasks.

### Contracts/interfaces introduced

`ModelGateway`, bounded stream accumulator, `AgentLoop` as `WorkRunner`, terminal message builder, and draft sink interface.

### Failure behavior

Provider retries are bounded/pre-output only; tool observed errors return to model; unknown tool outcomes interrupt; context/provider/loop/storage failures terminate honestly; cancellation prevents late dispatch/completion.

### Validation

Scripted full loops, retry/stream/cancel/limit matrices, multiple ordered tools, mixed text, refusal, final commit races, scheduler queue draining, and journal/projection/context reconstruction.

### Exit criteria

- [ ] One claimed work executes only through the explicit loop.
- [ ] Every invocation/tool/final answer has correct durable evidence/order.
- [ ] Scheduler owns/observes agent tasks and progresses queued followers.
- [ ] No provider state, socket, draft, or task is canonical.

### What is deliberately NOT implemented yet

Real OpenAI, live WebSocket drafts, parallel tools, automatic in-flight resume, sophisticated routing, memory, or native client.

### Substages

#### Substage 17.1: Implement Model Gateway attempt transactions, streaming, retries, and outcomes

##### Objective

Surround each provider attempt with durable intent/outcome and application-owned retry/cancellation rules.

##### Why it happens now

Prepared context manifests and provider port exist; the agent loop needs one safe inference service.

##### Preconditions

Selected target, fitting `ContextPackage`, prepared manifest/artifact, active runtime-owned work, and ScriptedProvider exist.

##### Exact implementation work

- For attempt 1, atomically insert manifest/sources, invocation `requesting`, set work `waiting_on_model/current`, append `model.invocation_started` and `work.waiting_on_model`, then commit before provider I/O.
- For retry attempts, reuse logical invocation/manifest, insert a new attempt/retry link and start event; never overwrite prior attempt.
- Stream through one bounded accumulator; record first byte/output times; forward only safe text/refusal deltas to an injected draft sink; buffer tool args and wait for complete response.
- Classify/record each observed completed/failed/cancelled/unknown result with usage/IDs/ordered output/error; atomically append terminal model event and `work.resumed`, unless cancellation or exhausted fatal failure wins.
- Apply initial+two retry policy only before semantic draft; interrupt backoff on cancellation; record delay/classification/billing ambiguity.

##### Data/state introduced

Manifest/source rows, invocation attempt rows/events, work waiting/resumed/failure state, usage/latency/retry/draft fields, optional request/response artifacts.

##### Contracts/interfaces introduced

Provider call starts only after intent commit; complete normalized output commits before any tool dispatch/final decision.

##### Failure behavior

Storage failure before call causes no call; transient pre-output may retry; after-output failure never auto-retries; unknown stream semantics fail closed; cancellation result cannot complete work.

##### Validation

All ScriptedProvider scenarios, pre/post intent markers, exact attempt rows/events, retry counts/delays, draft exposure flags, cancellation during call/backoff, and no tool call on partial args.

##### Exit criteria

- [ ] Every outbound attempt has a prior durable intent.
- [ ] Every retry is separately evidenced.
- [ ] Provider completion is durable before interpretation.
- [ ] Draft exposure disables automatic retry.

##### What is deliberately NOT implemented yet

OpenAI HTTP/SSE mapping, provider background jobs/conversations, fallback models, or provider-side cancellation certainty.

#### Substage 17.2: Implement the explicit bounded agent-loop algorithm

##### Objective

Drive one work item through selection, context, model, tools, continuation, and terminal decision using ordinary inspectable Rust control flow.

##### Why it happens now

All constituent services and pure decisions exist; composition can now be linear and testable.

##### Preconditions

Substage 17.1, Context Assembler, Tool Execution Service, cancellation coordinator, clocks, and limits exist.

##### Exact implementation work

- Reload/verify work ownership/state/version; check cancel/work deadline; derive required capabilities/cheap stats; select target; assemble/limit-check context; invoke Model Gateway.
- Inspect complete ordered response only after persistence. If tools exist, preserve order and process sequentially through Tool Service; do not start later call after cancellation/unknown outcome.
- After each observed tool result, increment step as defined and loop so new persisted evidence enters a fresh selected-target context manifest.
- If no executable call and terminal output is valid, build final user-visible text/structured/refusal outcome; otherwise classify incomplete/empty/unknown/limit/provider failure.
- Enforce 16 steps, 32 model attempts/work, 32 tool calls/work, 64 output items/response, argument/time/work limits with exact terminal codes.

##### Data/state introduced

Agent step/tool ordinals, repeated invocation/tool records, loop counters/timing, and terminal decision evidence.

##### Contracts/interfaces introduced

No library/handler recursively invokes the model; one scheduler task owns one loop; selection occurs on every step before context rendering.

##### Failure behavior

Observed tool error is eligible for next context; tool unknown outcome interrupts immediately; definite unrecoverable provider/context/limit/internal condition fails; cancellation follows cleanup certainty.

##### Validation

Scripted zero/one/many-step loops, text+tool, multiple tools, ordinary tool errors/recovery, unknown outcome, refusal, each limit, cancellation at every checkpoint, and exact call/order counts.

##### Exit criteria

- [ ] Loop control is explicit and bounded.
- [ ] Context is rebuilt from durable state each step.
- [ ] No hidden retry/recursive provider/tool execution exists.
- [ ] Terminal decision matches Stage 4 matrix.

##### What is deliberately NOT implemented yet

Parallel model/tool execution, subagents, planning frameworks, pause/steer, background responsibilities, or long-running sessions.

#### Substage 17.3: Implement final answer/refusal/failure/cancellation commits

##### Objective

Make user-visible terminal truth atomic with terminal work state and durable delivery facts.

##### Why it happens now

The loop can identify terminal output; the client must never see a committed answer unsupported by completed work.

##### Preconditions

Substage 17.2 has a persisted terminal invocation and active noncancelled work.

##### Exact implementation work

- Recheck durable cancellation/runtime ownership/current invocation before terminal commit.
- In one transaction insert immutable assistant message, append `assistant.message_committed` caused by terminal invocation, transition work to completed (`answered|refused`), append `work.completed`, clear attempts/ownership, and return cursor/message receipt.
- For definite no-answer failures append `work.failed` with normalized safe terminal detail and no fabricated assistant message unless architecture-approved user-facing failure projection is separate.
- For confirmed cancellation append attempt/tool evidence as applicable then `work.cancelled`; for ambiguity append unknown/interrupted events.
- Notify event delivery/scheduler only after commit; draft sink receives committed/abandoned relationship separately.

##### Data/state introduced

Assistant message and completion events, terminal work reason/time, or failed/cancelled/interrupted terminal events.

##### Contracts/interfaces introduced

Only final-answer transaction creates assistant conversation messages; streaming text and provider response evidence never do.

##### Failure behavior

Crash before commit leaves no message/completion and recovery interrupts; crash after commit is completed and replayable; cancellation race blocks completion.

##### Validation

Before/during/after commit kill hooks, late cancel/provider races, refusal content, duplicate final-call attempt, immutable message, exact event causation/order, and replay candidates.

##### Exit criteria

- [ ] Assistant message and completed work are inseparable.
- [ ] Draft/failure cannot become a committed message accidentally.
- [ ] Post-commit delivery loss is replay-safe.
- [ ] Terminal fields/current ownership are consistent.

##### What is deliberately NOT implemented yet

Message edits, partial final commits, provider-generated failure text as truth, or automatic continuation of interrupted work.

#### Substage 17.4: Wire AgentLoop into Scheduler and complete runtime readiness dependencies

##### Objective

Replace the test runner with the real loop and make readiness reflect a fully usable deterministic backend composition.

##### Why it happens now

The agent loop and all local dependencies are complete; scheduler integration can be tested without a real provider using ScriptedProvider.

##### Preconditions

Substages 17.1–17.3, startup recovery, registry, default scripted target, LocalWorkstation, and State/Artifact stores pass.

##### Exact implementation work

- Implement AgentLoop as scheduler `WorkRunner`; inject concrete State Store, ArtifactStore, selector/assembler/gateway/scripted provider, Tool Service/LocalWorkstation, event delivery/draft sink, clock/cancellation/limits.
- Start scheduler only after config/migrations/integrity/bootstrap/new runtime/recovery/registry/provider target/workstation capabilities are valid.
- Mark ready only after scheduler scan begins; mark unready/fatal on storage invariant or critical task ownership failure.
- On task terminal, notify scan for next FIFO work; on panic classify/interrupt and degrade health.
- Prove graceful shutdown propagates through provider/scripted waits and Workstation cleanup.

##### Data/state introduced

Production-like deterministic runtime tasks/readiness state and all normal work evidence.

##### Contracts/interfaces introduced

Composition root chooses adapters; scheduler/AgentLoop communicate through the narrow runner result, not shared SQL/process internals.

##### Failure behavior

Missing/unusable registry/target/workstation/storage leaves unready. Agent panic is observed and never silently strands running work.

##### Validation

Start fresh/reopen, submit multiple works/conversations fixtures, verify readiness order, FIFO progression, task panic, cancellation, SIGTERM, and zero detached tasks.

##### Exit criteria

- [ ] Deterministic backend becomes honestly ready.
- [ ] Real AgentLoop is the only production work runner.
- [ ] Queued work progresses after all terminal outcomes.
- [ ] Startup/shutdown order matches architecture.

##### What is deliberately NOT implemented yet

Live OpenAI readiness, draft WebSocket delivery, EC2/systemd supervision, or native product UI.

## Stage 18: Deterministic end-to-end responsibility spine and model-side crash suite

### Objective

Prove the entire headless product path with ScriptedProvider and real Ubuntu LocalWorkstation, then close context/model/provider/final-answer crash windows before adding external inference.

### Why it happens now

Every local subsystem is composed. Real provider variability must not be introduced until deterministic orchestration and recovery are evidence-backed.

### Preconditions

Stage 17 is ready with all earlier tool crash tests passing on Ubuntu.

### Exact implementation work

- Execute Substages 18.1–18.3.
- Use protocol commands/events, not internal repository shortcuts, for end-to-end tests.
- Retain redacted DB/trace/artifact evidence per run.

### Data/state introduced

Complete deterministic conversations/work/context/model/tool/assistant/replay histories and crash-recovery fixtures/evidence.

### Contracts/interfaces introduced

An executable headless acceptance harness becomes the baseline that every later adapter/client/deployment must pass unchanged.

### Failure behavior

Any missing intent/evidence, wrong context source, duplicate side effect, false terminal state, cursor gap, or process leak fails the gate; no tolerance is attributed to model nondeterminism.

### Validation

Scripted machine inspection, tool failure/recovery, multi-message queue, cancellation, duplicate, context limit, full failpoint matrix to date, reopen/replay/projector/integrity checks.

### Exit criteria

- [ ] Full deterministic responsibility spine passes through public protocol.
- [ ] All model/context/final crash windows are classified exactly.
- [ ] Ubuntu process/artifact evidence is complete.
- [ ] Later stages can compare against one fixed harness.

### What is deliberately NOT implemented yet

OpenAI, ephemeral WebSocket drafts, macOS app, TLS/EC2, backup restore, or release benchmark signoff.

### Substages

#### Substage 18.1: Pass the scripted machine-inspection and continuity path

##### Objective

Demonstrate the canonical orchestration using deterministic model outputs and actual Ubuntu observations.

##### Why it happens now

This is the first full integration of command, scheduler, context, model, tools, persistence, and replay.

##### Preconditions

Public protocol and deterministic ready runtime run on Ubuntu with Git installed.

##### Exact implementation work

- Script first response to request one or more real `run_shell`/`read_file` operations that observe OS, architecture, cwd, and Git version; script second response to produce a final answer from persisted results.
- Submit through authenticated HTTP, observe durable WebSocket progress/replay, and assert exact message/work/invocation/tool/assistant state and event ordering.
- Kill/reopen backend after completion, run startup recovery/new runtime, reconnect from cursor, submit the Git-version follow-up, and require context built from durable history with provider continuation disabled.
- Add ordinary missing-file/nonzero shell scenarios that return structured results and allow scripted model recovery.

##### Data/state introduced

Canonical deterministic benchmark conversation and evidence artifacts in disposable test state.

##### Contracts/interfaces introduced

The harness asserts semantics, not a hard-coded command sequence, while ScriptedProvider remains exact about expected context/result hashes.

##### Failure behavior

Machine fact mismatch, use of Mac/fake machine instead of Ubuntu, missing usage/context/tool evidence, or RAM/provider-state continuity fails.

##### Validation

Inspect database/event order/artifact hashes/traces, restart with empty provider memory, and independently compare observed OS/Git facts.

##### Exit criteria

- [ ] Actual Ubuntu facts flow through the complete product spine.
- [ ] Follow-up succeeds after process restart from durable history.
- [ ] Observed tool failures do not crash/falsify work.

##### What is deliberately NOT implemented yet

Model quality evaluation, real provider latency/tokens, native UI, or target EC2 topology.

#### Substage 18.2: Activate context/model/provider/final-answer failpoints

##### Objective

Prove rollback versus conservative interruption at every inference/final commit window.

##### Why it happens now

Model attempt transactions, scripted streams, and final commit are complete and deterministic.

##### Preconditions

Substage 18.1 and Stage 2 atomic failpoint definitions exist.

##### Exact implementation work

- Activate precise intra-transaction manifest-row and full model-attempt pre-commit hooks behind architecture aliases `after_context_manifest_commit`/`after_model_intent_commit`, plus actual post-attempt-commit-before-provider hook.
- Activate `after_first_provider_delta`, `after_model_response_commit`, and pre/post full final-answer transaction hooks behind `after_assistant_message_commit` semantics.
- For each: kill process, reopen/recover, inspect whether transaction rolled back or committed, abandon draft, mark invocation/provider outcome/work exactly per recovery policy, and assert no unintended provider retry/tool dispatch.
- When normalized response is committed but old active loop dies, follow frozen V0 policy: do not silently resume; mark work interrupted unless the explicitly defined committed-final transaction already completed.

##### Data/state introduced

Crash fixtures with requesting/streaming/completed attempts, interrupted work, abandoned drafts, committed final messages, and recovery events.

##### Contracts/interfaces introduced

Named hooks now have unambiguous physical locations/expected durable state without weakening atomic transactions.

##### Failure behavior

Partial manifest/intent/final rows, false completion, automatic post-crash continuation, or missing draft abandonment fails the stage.

##### Validation

Repeat every hook, run quick/foreign-key/projector checks, assert exact rows/events/cursors/provider/tool call counts, and reconnect clients after final commit.

##### Exit criteria

- [ ] All required inference/final failpoints pass.
- [ ] Atomic transaction challenge is resolved in executable tests.
- [ ] No arbitrary old loop resumes.
- [ ] Committed final answer is replayed without reexecution.

##### What is deliberately NOT implemented yet

Live-network crash semantics, provider request reconciliation, or randomized failpoint combinations.

#### Substage 18.3: Freeze the deterministic integration contract and evidence schema

##### Objective

Make the deterministic suite the nonregression gate for OpenAI, streaming, client, and deployment work.

##### Why it happens now

The local correctness spine is complete; later stages should add adapters/surfaces rather than alter semantics.

##### Preconditions

Substages 18.1–18.2 and all prior unit/integration tests pass.

##### Exact implementation work

- Define one command that creates disposable state/workspace, launches backend, provisions device, runs the deterministic scenarios/failpoints, checks DB/artifacts/processes/replay, and emits a redacted result manifest.
- Freeze golden protocol/event/context/model/tool fixtures at protocol/schema/config versions.
- Record required query assertions for counts, order, usage, context contributions, privilege, output, cleanup, interruption, recovery, and cursor delivery.
- Add boundary lint checks for SQLx/OpenAI/Command/filesystem/WebSocket misuse and release failpoint absence.

##### Data/state introduced

Versioned deterministic fixtures and evidence manifests outside product state.

##### Contracts/interfaces introduced

Any later schema/protocol/domain change must update fixtures through explicit compatibility review, not silent snapshot churn.

##### Failure behavior

Fixture drift without approved semantic reason or a nondeterministic test fails CI/release gate.

##### Validation

Run twice from empty and once after reopen; compare normalized evidence (excluding allowed timestamps/IDs) and prove isolation/cleanup.

##### Exit criteria

- [ ] One reproducible local/Ubuntu deterministic gate exists.
- [ ] Evidence covers every architecture claim implemented so far.
- [ ] Later adapters can be substituted without bypasses.

##### What is deliberately NOT implemented yet

External credentials/network tests, snapshot backups, performance optimization, or release acceptance waivers.

## Stage 19: OpenAI Responses API adapter and live provider smoke path

### Objective

Implement the only real V0 provider using Reqwest and the current Responses API while preserving Craxii's stateless canonical history, ordered output, custom tools, streaming, usage, errors, and secret boundary.

### Why it happens now

The deterministic provider boundary and full agent spine are frozen. OpenAI becomes a replaceable edge rather than defining runtime semantics.

### Preconditions

Stage 18 passes; a dedicated spend-limited OpenAI development project/key and capable configured model are available for live smoke tests.

### Exact implementation work

- Execute Substages 19.1–19.4.
- Re-fetch official OpenAI documentation and verify the chosen model/fields immediately before implementation; pin redacted fixtures to observed current wire behavior.
- Keep all OpenAI structs/auth/event parsing under `adapters/openai`.

### Data/state introduced

Provider-native typed options/wire fixtures, encrypted/opaque continuation artifacts, provider IDs/status/usage/latency/error evidence, request/response diagnostic artifacts, and live smoke-test records.

### Contracts/interfaces introduced

OpenAI implementation of `ModelProvider` and estimator, SSE decoder, request translator, response normalizer, and safe provider-error classifier.

### Failure behavior

Auth/permission/invalid/schema/context/protocol errors fail definite/nonretry as classified; transient pre-output failures may be retried only by Model Gateway; unknown correctness-bearing events fail closed.

### Validation

Local HTTP/SSE fixtures, official-schema field assertions, ordered function-call round trips, retry/error/usage/continuation tests, sentinel secret scans, and an opt-in live stateless tool-call smoke test.

### Exit criteria

- [ ] OpenAI wire types do not leak outside adapter.
- [ ] `store=false`, full Craxii context, custom tools, no conversation correctness dependency, and no partial execution are proven.
- [ ] Ordered output/usage/IDs/errors normalize completely.
- [ ] Live smoke passes with current configured model.

### What is deliberately NOT implemented yet

Second provider, provider built-in tools, provider conversation/previous-response correctness dependency, automatic model fallback, background Responses jobs, or native client release path.

### Substages

#### Substage 19.1: Implement secret-safe Reqwest client and request translation

##### Objective

Build a bounded authenticated Responses request from one canonical `ModelRequest` without exposing provider secrets or state.

##### Why it happens now

Canonical request/tool/target semantics are frozen and can be translated one-way inside the adapter.

##### Preconditions

Validated endpoint/model/native options and loaded redacted OpenAI credential reference exist.

##### Exact implementation work

- Load key through secret source into nonprintable wrapper used only to create authorization header inside adapter; configure TLS/connect/idle/overall limits and bounded response handling.
- Translate instructions/ordered input items/custom `read_file`/`run_shell` function schemas/tool results/output limit/tool choice/provider-native typed options.
- Explicitly set `store=false`, `parallel_tool_calls=false` when supported, and disabled/no-auto truncation semantics; send full context each invocation; omit `conversation` and `previous_response_id` as correctness mechanisms.
- Request encrypted reasoning continuation inclusion when chosen reasoning target/stateless loop requires it; never request built-in web/file/computer/shell/MCP tools.
- Hash/store only approved redacted request evidence and request byte count; never log body/header/key.

##### Data/state introduced

Ephemeral HTTP requests and optional redacted request artifact; invocation row later stores hashes/options/bytes.

##### Contracts/interfaces introduced

OpenAI-specific fields are adapter-owned and selected through typed target options, never client/model arbitrary JSON.

##### Failure behavior

Missing key/unsupported current field/model/tool schema/oversized request fails before send where known. HTTP uncertainty is classified for Gateway, not retried in Reqwest middleware.

##### Validation

Capture requests against local server and assert every required/forbidden field/header/body behavior, limits/timeouts, content order, secret absence, and exact tool schemas.

##### Exit criteria

- [ ] Stateless full-context request is exact.
- [ ] Only custom Craxii tools are offered.
- [ ] Secret exists only inside adapter/header construction.
- [ ] Transport performs no hidden retry.

##### What is deliberately NOT implemented yet

Provider SDK, conversation objects, hosted tools, file upload/vector stores, batch/background modes, or client-supplied native options.

#### Substage 19.2: Implement bounded SSE decoding and complete argument assembly

##### Objective

Turn Responses streaming events into safe canonical stream events while preserving item/sequence order and never dispatching partial calls.

##### Why it happens now

Request path exists; streaming is required for drafts/latency and complete output normalization.

##### Preconditions

Substage 19.1 local fixture server and canonical accumulator contract exist.

##### Exact implementation work

- Parse SSE framing incrementally with byte/event/idle/overall bounds; handle response created/queued/in-progress, item added, text/refusal/argument deltas, item done, completed/incomplete/failed, usage/IDs.
- Track provider sequence/item indices/order, first byte/semantic output, duplicate/out-of-order events, and bounded raw diagnostic evidence for unsupported events.
- Accumulate function arguments by call/item ID up to 64 KiB; accept only final item + terminal response; validate UTF-8/JSON later against registry schema.
- Emit safe text/refusal deltas to Gateway; do not expose reasoning secrets, raw events, incomplete args, or tool calls.
- Cancel/read-close cleanly and classify whether semantic output escaped.

##### Data/state introduced

Ephemeral stream accumulator/events/timings and optional bounded diagnostic artifact for unknown events.

##### Contracts/interfaces introduced

SSE sequence is provider evidence, not journal/client cursor; complete normalized response is the only tool-dispatch input.

##### Failure behavior

Malformed SSE, duplicate/conflicting item/call, limit/idle timeout, terminal error, or unknown semantic event becomes classified provider failure; no partial tool action.

##### Validation

Fragmented/multiline SSE, every event fixture, reordered/duplicate/unknown events, partial/malformed/oversized args, disconnect before/after delta, idle timeout, cancellation, and accumulator memory bound.

##### Exit criteria

- [ ] Ordered stream reconstructs exact output.
- [ ] Partial args can never reach Tool Service.
- [ ] Unknown semantic events fail closed with bounded evidence.
- [ ] Latency markers are captured.

##### What is deliberately NOT implemented yet

Raw provider event forwarding to client, tool execution during stream, resumable provider stream, or unbounded diagnostic retention.

#### Substage 19.3: Normalize ordered output, usage, continuation, IDs, and errors

##### Objective

Map one complete Responses outcome into canonical types with enough evidence for context continuation and observability.

##### Why it happens now

SSE accumulation is complete; the provider boundary must close before live use.

##### Preconditions

Substage 19.2 and canonical ordered response/error types exist.

##### Exact implementation work

- Preserve response output array order and nested content order across text, function call, refusal, structured/native/reasoning items; reject correctness-bearing unknowns.
- Parse completed tool call IDs/names/raw+parsed args; retain provider response/request IDs, status/incomplete/stop detail, model actually served, and bounded metadata.
- Parse input/cached/output/reasoning/total usage and service/cache fields where current API reports them; leave unavailable values null rather than infer.
- Store encrypted reasoning continuation as provider-guarded opaque bytes/artifact with source/hash/eligibility; permit reinsertion only for compatible OpenAI target and never as sole history.
- Map HTTP/SSE/provider errors/status/retry guidance to normalized classifications with redacted detail.

##### Data/state introduced

Canonical completed response/usage/continuation/error and provider evidence ready for invocation outcome transaction.

##### Contracts/interfaces introduced

Common semantics remain accessible without native bytes; actual provider identifiers are evidence, not domain identity.

##### Failure behavior

Missing required terminal output/usage shape where necessary, invalid call arguments, incompatible continuation, or unsupported semantics fails definite and nonretry after output.

##### Validation

Golden redacted response fixtures for mixed items, multiple calls, refusal, incomplete/failed, usage variants, encrypted continuation, IDs, unknown item, and error/status/retry mapping.

##### Exit criteria

- [ ] Output order and all V0 item types survive normalization.
- [ ] Usage/IDs/errors are persisted when observed and never fabricated.
- [ ] Continuation remains optional/provider-scoped.
- [ ] No raw response struct escapes adapter.

##### What is deliberately NOT implemented yet

Global reasoning interpretation, cost guessed from internet pricing, provider conversation retrieval, or cross-provider continuation translation.

#### Substage 19.4: Complete contract fixtures and one live stateless smoke test

##### Objective

Prove current wire compatibility and real custom-tool continuation without making live nondeterminism a correctness suite.

##### Why it happens now

Request/stream/normalization/error paths are complete under fixtures; one real call detects API drift/account/model limitations.

##### Preconditions

Dedicated key/model/account limits and safe Ubuntu test environment are available; deterministic suite remains green.

##### Exact implementation work

- Recheck official API/model documentation and record verification date/model snapshot/fields/context/tool/stream support in target config/dependency decision.
- Run a minimal headless call that emits a custom tool request, persist it, return a deterministic safe tool result, then obtain final text with `store=false` and no provider conversation dependency.
- Verify provider request/response IDs, usage, first byte/output/total latency, tool args, retries (normally zero), and continuation evidence where enabled.
- Run with provider continuation disabled/omitted to prove full durable context correctness; optionally delete/unretrieve stored response expectation because `store=false`.
- Sanitize and save wire fixtures only if license/security policy permits; never save key/full sensitive content.

##### Data/state introduced

Live development invocation/tool/final records and a redacted smoke evidence manifest.

##### Contracts/interfaces introduced

Live smoke is opt-in for normal CI but mandatory for release and current-wire verification.

##### Failure behavior

Unavailable model/key/rate limit is a configuration prerequisite failure, not architecture failure. Schema/semantic mismatch blocks this stage until adapter/config or explicit architecture amendment is resolved.

##### Validation

Inspect request capture safely, DB rows/events/context/artifacts/traces, provider dashboard/request IDs where available, and exact call counts.

##### Exit criteria

- [ ] Current live Responses tool loop succeeds once.
- [ ] Stateless correctness is proven.
- [ ] Required usage/latency/ID evidence is queryable.
- [ ] Deterministic suite still passes unchanged.

##### What is deliberately NOT implemented yet

Quality benchmarking, production traffic/credentials, multiple models/providers, load testing, or final canonical product benchmark.

## Stage 20: Ephemeral draft streaming and complete live event delivery

### Objective

Add safe best-effort assistant drafts and high-level tool progress to the already-correct durable replay channel, with abandonment, committed replacement, reconnect, and backpressure semantics.

### Why it happens now

Durable replay was proven first and real provider streaming exists. Draft UX can now remain explicitly subordinate to committed facts.

### Preconditions

Stages 11, 17, and 19 pass; draft sink and model invocation `draft_exposed` evidence exist.

### Exact implementation work

- Execute Substages 20.1–20.3.
- Never journal token deltas or make delivery affect work execution.
- Keep tool stdout streaming deferred; expose only durable high-level tool start/finish projections.

### Data/state introduced

Ephemeral draft IDs/start/delta/abandon events and per-connection draft buffers/sequences; durable public tool/work/message projections remain journal-derived.

### Contracts/interfaces introduced

Draft lifecycle sink, WebSocket ephemeral envelope, committed-message reconciliation relationship, and coalesce/drop policy distinct from durable queue policy.

### Failure behavior

Draft/socket loss affects presentation only. Failure/cancel/retry after an exposed draft emits abandonment where connected; reconnect discards old drafts even if abandonment was missed.

### Validation

Streaming text/tool mixes, exposed-output retry prohibition, disconnect/reconnect during every draft phase, committed replacement, failure/cancel abandonment, coalescing, slow clients, and durable replay convergence.

### Exit criteria

- [ ] Drafts are visibly/noncanonically identified and never replayed.
- [ ] Committed message replaces draft authoritatively.
- [ ] Reconnect cannot concatenate stale/retried drafts.
- [ ] Slow/dropped draft delivery cannot block persistence/work.

### What is deliberately NOT implemented yet

Durable token streams, tool stdout streaming, draft recovery after reconnect, typing collaboration, or client UI.

### Substages

#### Substage 20.1: Implement draft lifecycle and safe provider-to-delivery projection

##### Objective

Convert safe model text/refusal deltas into explicitly ephemeral events associated with one work/invocation/draft.

##### Why it happens now

Gateway knows complete invocation identity and whether semantic output has escaped; WebSocket already has a noncanonical envelope.

##### Preconditions

Draft sink interface, OpenAI/ScriptedProvider stream events, and event-delivery manager exist.

##### Exact implementation work

- Generate one draft ID per exposed invocation and monotonic per-draft delta sequence; emit started before first delta.
- Project only safe user-visible text/refusal deltas with work/conversation/invocation IDs; exclude tool args/reasoning/raw provider data/secrets.
- Mark invocation `draft_exposed` through its terminal persistence evidence; tell retry policy immediately when first semantic delta is delivered to any sink.
- Emit `draft_abandoned` on provider failure/cancel/nonterminal text-with-tools/new attempt where connected; on final commit emit relationship so client replaces draft.
- Bound per-draft bytes/events and coalesce adjacent text deltas without changing committed content.

##### Data/state introduced

Ephemeral draft events/buffers; durable invocation exposed flag and metrics only.

##### Contracts/interfaces introduced

Draft identity is invocation-scoped and disposable; terminal assistant message content comes only from normalized complete response/final transaction.

##### Failure behavior

Draft sink backpressure/drop does not fail model/work. If semantic output was offered to sink, retry remains disabled conservatively even if no client observed it.

##### Validation

No-delta/text/refusal/text+tool/failure/cancel/limit sequences, byte/event bounds, ordering/coalescing, sensitive item filtering, and exposed-flag/retry tests.

##### Exit criteria

- [ ] Every draft has start/ordered deltas/terminal abandon-or-commit relation when connected.
- [ ] Draft content cannot dispatch a tool or commit a message.
- [ ] Retry policy sees exposure deterministically.

##### What is deliberately NOT implemented yet

Draft persistence/replay, token-level exactness guarantees, reasoning display, tool arguments/stdout, or editing drafts.

#### Substage 20.2: Integrate drafts with replay/live handoff, reconnect, and committed events

##### Objective

Ensure ephemeral traffic never races durable catch-up or corrupts client convergence.

##### Why it happens now

Draft lifecycle is defined and the durable handoff algorithm is already correct.

##### Preconditions

Substage 20.1 and Stage 11 replay/live connection state exist.

##### Exact implementation work

- Deliver drafts only after `sync.complete`; allow drafts produced during catch-up to be missed by design.
- Prioritize/query durable journal events independently of ephemeral queue; committed assistant event contains full authoritative content and work/message identity.
- On connection close drop all connection draft buffers; on reconnect do not replay them and signal client protocol to discard pre-disconnect drafts.
- Project durable tool requested/dispatch/completed into safe high-level `tool.execution_started/finished` events with bounded summaries/privilege/result, not raw command/output.
- Handle commit arriving while draft deltas are queued by ordering/reconciliation IDs, not assuming last socket frame wins.

##### Data/state introduced

Combined per-connection durable/ephemeral delivery state and reconciliation metadata.

##### Contracts/interfaces introduced

Durable queue is lossless-or-disconnect; ephemeral queue is coalescible/droppable; committed event wins regardless of arrival timing.

##### Failure behavior

Ephemeral overflow drops/coalesces drafts and records metric. Durable overflow closes/replays. Missing abandonment after disconnect is repaired by client discard/bootstrap.

##### Validation

Commit during catch-up, draft during replay, queued delta then commit, reconnect without abandonment, tool progress redaction, broadcast gaps, and multiple fast/slow clients.

##### Exit criteria

- [ ] Drafts never precede sync completion on a new connection.
- [ ] Committed message appears exactly once in converged projection.
- [ ] Durable events retain priority and gap repair.
- [ ] Tool progress exposes no raw sensitive evidence.

##### What is deliberately NOT implemented yet

Durable progress logs, artifact download, exact live tool output, multi-device draft synchronization, or cursor on ephemeral events.

#### Substage 20.3: Complete live protocol telemetry and contract tests

##### Objective

Freeze the full backend protocol—including drafts—as a tested client-ready contract.

##### Why it happens now

The native app should consume stable fixtures rather than discover races while UI code is being written.

##### Preconditions

Substages 20.1–20.2 and all protocol fixtures pass.

##### Exact implementation work

- Extend language-neutral fixtures with draft lifecycle, tool progress, recovery, interruption, cancellation, slow-client close, and sync events.
- Record connect/disconnect/reconnect, replay count/lag, slow disconnect, durable query recovery, dropped/coalesced drafts, auth/version failure, and command commit-to-response metrics.
- Run a simulated Swift-like projection client against randomized duplicate/delayed durable frames and dropped drafts to prove convergence.
- Verify max 256 KiB durable payload and bounded ephemeral frames; large evidence always stays in artifacts/summaries.

##### Data/state introduced

Final protocol v1 fixture corpus and delivery metrics.

##### Contracts/interfaces introduced

The backend protocol is now ready for independent platform clients; fixtures are compatibility evidence.

##### Failure behavior

Unprojectable event or fixture drift blocks client stage. Unknown future optional payload fields remain ignorable; unknown version remains explicit failure.

##### Validation

Run complete auth/command/bootstrap/replay/live/draft/backpressure/reconnect test matrix and redaction scan.

##### Exit criteria

- [ ] Protocol fixtures cover every client-visible state/event.
- [ ] Delivery/reconnect metrics are queryable.
- [ ] A stateless simulated client converges under duplicates/gaps/draft loss.

##### What is deliberately NOT implemented yet

Native app, protocol v2 negotiation, retained event epochs, push notifications, or web/mobile clients.

## Stage 21: Native macOS project, transport, credentials, and client projection core

### Objective

Create the real native app target and a tested non-UI client core that stores the device token in Keychain, submits idempotent HTTP commands, bootstraps/reduces durable state, tracks cursors, consumes WebSocket replay/drafts, and reconnects safely.

### Why it happens now

The full backend protocol and fixtures are stable. Building the client earlier would have made UI state define unresolved command/replay semantics.

### Preconditions

Stage 20 passes; current Xcode and the minimum supported macOS version/signing approach are selected; a development backend/device token exists.

### Exact implementation work

- Execute Substages 21.1–21.4.
- Prefer Apple URLSession/URLSessionWebSocketTask and Security/Keychain APIs unless an approved dependency solves a demonstrated gap.
- Keep all client caches/outbox data disposable and namespaced by backend Craxii ID/protocol version.

### Data/state introduced

Xcode project/targets, Swift protocol models/fixtures, Keychain token item, nonsecret endpoint settings, disposable pending-command/cursor/projection cache, reconnect state, and client telemetry.

### Contracts/interfaces introduced

`CraxiiHTTPClient`, `CraxiiEventStreamClient`, `KeychainDeviceCredentialStore`, `ReconnectController`, durable event reducer, and `@MainActor ConversationStore` input boundary.

### Failure behavior

Transport loss never marks work failed or generates a new command ID. Decode/version/auth uncertainty triggers reconnect/bootstrap or explicit user-visible configuration error; committed backend state wins local cache.

### Validation

Swift unit tests against backend JSON fixtures/URLProtocol/fake WebSocket streams, Keychain tests, stable UUID/idempotency tests, cursor duplicate/gap/reconnect tests, draft discard, and real localhost integration.

### Exit criteria

- [ ] Native app builds and test targets run.
- [ ] Raw device token exists only in Keychain and request header construction.
- [ ] HTTP retries reuse exact command IDs/material.
- [ ] Snapshot+events converge under duplicates/gaps/disconnects.

### What is deliberately NOT implemented yet

Product UI beyond a diagnostic shell, multiple accounts/conversations, provider settings/key, web view, cloud sync, push notifications, or release distribution.

### Substages

#### Substage 21.1: Create Xcode project, targets, configuration, and Swift protocol models

##### Objective

Establish a native SwiftUI app with independently testable transport/projection modules and exact protocol v1 decoding.

##### Why it happens now

Backend fixtures are frozen and can drive the client without duplicating architecture discovery.

##### Preconditions

Minimum macOS/development team choices and repository quality rules exist.

##### Exact implementation work

- Create `clients/macos/Craxii.xcodeproj` with app, unit-test, and UI-test targets; use SwiftUI app lifecycle and AppKit only for demonstrable native gaps.
- Organize protocol/transport/storage/reducer/view-model/view layers with inward dependencies; keep generated/build/user data ignored.
- Define Codable protocol v1 IDs/content/work/message/event/error/bootstrap/draft types, `Int64` cursor handling, strict required enum/version behavior, and additive optional fields.
- Import language-neutral backend fixtures into Swift tests without hand-editing semantic copies.
- Configure local/development endpoint through nonsecret app settings; enforce HTTPS/WSS outside explicit localhost debug mode.

##### Data/state introduced

Xcode project metadata, target settings, Swift protocol values, fixture resources, and app bundle identity.

##### Contracts/interfaces introduced

Swift types mirror public protocol only; no OpenAI/SQLite/Linux type or behavior enters the app.

##### Failure behavior

Unsupported protocol/required enum/cursor overflow/malformed payload becomes a typed sync error and triggers safe bootstrap/update guidance, never partial state application.

##### Validation

Build all targets, decode/encode every fixture, unknown optional/required/version tests, cursor extremes, and static dependency review.

##### Exit criteria

- [ ] Native app/test targets compile reproducibly.
- [ ] Every public fixture has a Swift test.
- [ ] Nonlocalhost cleartext is disallowed.
- [ ] Client layer boundaries are explicit.

##### What is deliberately NOT implemented yet

Cross-platform shared SDK, Swift package publication, UI design system, signing/notarization pipeline, or server-generated client code.

#### Substage 21.2: Implement Keychain credentials and noncanonical local state

##### Objective

Protect the device bearer token and retain enough disposable client state for safe retries/reconnect without becoming authoritative.

##### Why it happens now

Transport cannot be tested against authenticated endpoints until credential and namespace behavior is defined.

##### Preconditions

App bundle/service identifiers and provisioned device token are available.

##### Exact implementation work

- Store/retrieve/delete token as a Keychain generic-password item scoped by backend endpoint/device; never place it in UserDefaults, plist, logs, crash metadata, source, or request fixtures.
- Store nonsecret endpoint/device display and last applied durable cursor in disposable local preferences/cache namespaced by backend Craxii ID + protocol version.
- Persist a small local pending-command outbox containing client ID, normalized content/hash, attempt state, and endpoint namespace—but no auth secret—until HTTP acknowledgement/bootstrap reconciliation.
- Define cache reset/revoked-token/protocol-mismatch flows; bootstrap always replaces/reconciles cached projection.
- Generate UUIDv7 stable client message/command IDs before first send using shared test vectors; never derive server IDs.

##### Data/state introduced

Keychain token and disposable local endpoint/cursor/outbox/projection metadata.

##### Contracts/interfaces introduced

Keychain is credential storage; local cache/outbox is a retry/presentation aid and may be deleted without corrupting server truth.

##### Failure behavior

Keychain denied/missing/revoked blocks authenticated transport with setup status. Cache corruption causes reset/bootstrap, not backend mutation or new resend ID.

##### Validation

Keychain add/update/read/delete/access tests, sentinel disk/log scan, outbox restart/retry, cache namespace switch/reset, UUID/hash cross-language fixtures.

##### Exit criteria

- [ ] Token is absent from ordinary filesystem/preferences/logs.
- [ ] Pending retry survives app restart with same ID/material.
- [ ] Cache loss recovers through bootstrap.

##### What is deliberately NOT implemented yet

Device enrollment/recovery UX, biometric gating, iCloud Keychain sync, user accounts, or provider credentials.

#### Substage 21.3: Implement HTTP commands, bootstrap, and durable reducer

##### Objective

Submit/retry commands and project authoritative snapshot/events idempotently in client memory.

##### Why it happens now

Protocol/credential/local IDs are stable; WebSocket can later feed the same reducer.

##### Preconditions

Substages 21.1–21.2 and backend HTTP fixture server exist.

##### Exact implementation work

- Implement authenticated message/cancel/bootstrap requests with protocol/version/content type/body bounds, matching Idempotency-Key, request IDs, and status/error decoding.
- Retry only retryable transport failures with original ID/material; never regenerate message/cancel ID; reconcile lost response through exact server receipt/bootstrap.
- Implement pure durable reducer keyed by event ID/cursor/message/work IDs; reject regression/incompatible gap assumptions while allowing cursor jumps over internal events.
- Apply bootstrap atomically to local projection and set snapshot cursor; reconcile optimistic/outbox entries by client/server IDs/content hash.
- Persist last fully applied cursor only after reducer success; surface revoked/auth/protocol errors distinctly.

##### Data/state introduced

Pending/acknowledged command projections, authoritative message/work state, applied-event IDs/window, and last durable cursor in disposable cache.

##### Contracts/interfaces introduced

HTTP acknowledgement moves optimistic input to committed/queued truth; reducer is the sole mutation path for server facts.

##### Failure behavior

Ambiguous network completion keeps outbox retryable under same ID. Conflict/auth/version errors do not auto-resend. Reducer failure forces bootstrap rather than guessing.

##### Validation

URLProtocol tests for success/lost response/duplicate/conflict/401/404/409/5xx/timeout, concurrent commands, bootstrap reconciliation, duplicate/out-of-order/cursor-jump events, and app-restart outbox.

##### Exit criteria

- [ ] Exact retry semantics match backend.
- [ ] Optimistic state never becomes canonical without ack/bootstrap/event.
- [ ] Cursor advances only after successful durable apply.
- [ ] Cancellation uses a fresh stable command ID.

##### What is deliberately NOT implemented yet

Automatic resend with changed content, WebSocket/drafts, message editing, offline autonomous queueing beyond explicit pending outbox, or UI.

#### Substage 21.4: Implement WebSocket sync, reconnect, draft state, and app lifecycle transport

##### Objective

Maintain live delivery while converging through bootstrap/cursor replay and treating drafts as disposable.

##### Why it happens now

HTTP/bootstrap/reducer are correct and can recover any WebSocket failure.

##### Preconditions

Substage 21.3 and full backend event fixtures exist.

##### Exact implementation work

- Open authenticated WSS with `after=last_cursor`; require replay followed by `sync.complete` before accepting/displaying drafts.
- Feed durable frames through the same reducer; deduplicate IDs/cursors, allow internal cursor jumps, persist cursor after apply, and request bootstrap on uncertain projection/protocol mismatch.
- Track draft ID/invocation/work/delta sequence separately; discard every draft on disconnect/background reset, abandon event, failure/cancel, or committed assistant replacement.
- Implement bounded exponential reconnect with full jitter and reset after stable sync; handle app background/foreground/network reachability without marking backend work failed or resubmitting messages.
- Treat slow-client/server close as replay trigger; handle ping/pong only as connection health, never product state.

##### Data/state introduced

Ephemeral socket/sync/backoff/draft buffers and connection status; durable cursor cache continues.

##### Contracts/interfaces introduced

Reconnect begins from last applied durable cursor; committed reducer state and drafts never share authority.

##### Failure behavior

Socket loss clears drafts and schedules reconnect. Decode/gap uncertainty triggers bootstrap. Reconnect never creates HTTP commands.

##### Validation

Fake stream scenarios for every replay/live race, duplicates, gaps, drops, sync gating, stale draft, commit replacement, slow close, background/foreground, jitter clock, and real localhost socket.

##### Exit criteria

- [ ] Durable state converges after arbitrary socket loss.
- [ ] All stale drafts disappear on reconnect.
- [ ] App lifecycle cannot cancel or duplicate backend work implicitly.
- [ ] Connection status is presentation-only.

##### What is deliberately NOT implemented yet

Background push, durable draft cache, tool stdout, multiple backend profiles active simultaneously, or UI polish.

## Stage 22: Native macOS conversation experience and client tests

### Objective

Deliver the actively usable one-conversation SwiftUI product surface with composition, pending/queued/active/tool/draft/committed/failed/cancelled/interrupted states, cancel control, reconnect visibility, and accessible native behavior.

### Why it happens now

The transport/reducer owns all correctness. UI can render state without inventing protocol semantics.

### Preconditions

Stage 21 passes against backend fixtures and localhost.

### Exact implementation work

- Execute Substages 22.1–22.4.
- Use one primary conversation with no thread picker.
- Ensure every destructive/retry-looking UI action maps to an explicit existing command contract.

### Data/state introduced

View state, composer text, selection/scroll/accessibility state, optimistic bubbles, tool progress cards, draft views, connection banners, and cancellation controls; all are noncanonical.

### Contracts/interfaces introduced

`@MainActor ConversationStore` publishes reducer-derived view models and accepts compose/send/cancel/retry-transport actions only.

### Failure behavior

UI never recasts disconnected as failed, unknown as failed, draft as committed, cancel-requested as cancelled, or pending as accepted. Retry uses stored command identity/material.

### Validation

Store unit tests, SwiftUI snapshot/accessibility tests where practical, UI automation with fixture backend, keyboard/scroll/composer behavior, every state rendering, reconnect/cancel flows, and memory/thread-safety checks.

### Exit criteria

- [ ] User can open app, see Craxii, send work, observe queue/progress/draft/final, cancel, and reconnect.
- [ ] Failure/interruption/unknown are visually distinct.
- [ ] Committed server state always wins.
- [ ] Native client suite passes without real provider dependency.

### What is deliberately NOT implemented yet

Conversation picker, accounts/teams/billing, rich attachments, settings beyond endpoint/device setup, web/mobile clients, production polish, or App Store release.

### Substages

#### Substage 22.1: Implement ConversationStore and transcript/composer shell

##### Objective

Render one continuous Craxii relationship and accept a stable identified message command.

##### Why it happens now

Transport/reducer are complete and can be wrapped by a main-actor store.

##### Preconditions

Stage 21 client services can be injected/faked.

##### Exact implementation work

- Build `ConversationStore` around bootstrap/event reducer, connection controller, pending outbox, and transport actions with main-actor isolation.
- Create SwiftUI app/window, conversation transcript, multiline composer, send keyboard/button behavior, disabled/length states, and startup/loading/empty states.
- Generate/store client message ID before first send and show optimistic “sending” bubble distinct from accepted message.
- Reconcile acknowledgement/bootstrap/event to stable message/work identities; preserve scroll/readability under streaming and queued updates.
- Show Craxii display identity from bootstrap, not hard-coded model/provider/hostname.

##### Data/state introduced

Main-actor view models, composer/pending/scroll state.

##### Contracts/interfaces introduced

Views call store intents; only reducer/network services mutate server projection; app displays one principal/primary conversation.

##### Failure behavior

Send validation stays local; transport ambiguity leaves retryable pending state with same ID; app never duplicates by reconstructing a new request silently.

##### Validation

Store concurrency/main-thread tests, composer length/multiline/keyboard, pending/ack/retry/restart, bootstrap identity, and transcript ordering.

##### Exit criteria

- [ ] One-message submission is usable and idempotent.
- [ ] Pending versus committed is unambiguous.
- [ ] Identity comes from backend bootstrap.

##### What is deliberately NOT implemented yet

Markdown-rich attachments, voice, multiple windows/conversations, model picker, or direct tool controls.

#### Substage 22.2: Render queued/running/model/tool and cancellation states

##### Objective

Make durable work progression and user cancellation understandable without exposing internal orchestration controls.

##### Why it happens now

The transcript shell is correct and public work/tool projections are available.

##### Preconditions

Substage 22.1 and work/tool event fixtures exist.

##### Exact implementation work

- Render queued ordinal/status immediately after command acceptance; running/waiting-on-model high-level progress; tool name/start/finish, requested/effective privilege where safe, duration/result/truncation summary.
- Add cancel control for queued/active states; create/reuse one cancel command per tap/retry, show `cancel_requested` cleanup pending, and disable new cancellation after terminal state.
- Continue accepting new messages while work active and show later work queued; do not label it as steering/merged.
- Render cancellation completion only from durable event/bootstrap; retain outcome-unknown/interrupted warnings if cleanup cannot be proven.

##### Data/state introduced

Work/tool progress view models and cancel-command UI state.

##### Contracts/interfaces introduced

UI submits responsibility/cancel only; it never selects a model/tool/privilege or assumes process state.

##### Failure behavior

Cancel transport loss retries same command. Active work remains active/disconnected until durable truth arrives. Tool nonzero is shown as observed result, not app/backend crash.

##### Validation

Queued follower, running/wait states, user/admin tool, nonzero/timeout/cancel/unknown, cancel duplicate/lost response/terminal no-op, and offline/reconnect UI tests.

##### Exit criteria

- [ ] Every public work/tool state has an honest presentation.
- [ ] Cancel lifecycle matches backend certainty.
- [ ] New messages queue during active work without steering language.

##### What is deliberately NOT implemented yet

Approval prompts, process terminal, live stdout, tool argument editing, priority/reordering, or pause/resume.

#### Substage 22.3: Render drafts, committed reconciliation, failures, and recovery

##### Objective

Show responsive output while keeping noncanonical drafts and uncertain outcomes unmistakable.

##### Why it happens now

Work/tool UI and draft reducer are present; terminal/recovery states need final product semantics.

##### Preconditions

Substages 22.1–22.2 and draft/failure/recovery fixtures exist.

##### Exact implementation work

- Render draft with transient visual treatment and invocation/work association; replace entire draft with committed message on authoritative event.
- Discard/mark abandoned on failure/cancel/disconnect/new attempt; never concatenate retry draft or persist as transcript.
- Render refused completion, definite failed work, cancelled work, interrupted work, and tool `outcome_unknown` with distinct text/icon/accessibility labels.
- Show recovery/reconnect summary without internal stack/provider data; allow later user follow-up as new message, not “resume” button.
- Ensure full committed message from replay replaces any partial UI state exactly once.

##### Data/state introduced

Draft/terminal/recovery presentation state only.

##### Contracts/interfaces introduced

Message ID/committed event defines transcript truth; work state defines terminal banner; unknown remains explicitly neither success nor failure.

##### Failure behavior

Missing draft frames causes only less live text. Duplicate committed event deduplicates. Unknown terminal/public event forces sync error/bootstrap rather than misleading UI.

##### Validation

Every draft/commit ordering, disconnect/restart, refusal/failure/cancel/interrupted/unknown/recovery event, duplicate replay, and accessibility snapshot.

##### Exit criteria

- [ ] Draft cannot survive reconnect or masquerade as message.
- [ ] All terminal certainties are distinct.
- [ ] Recovery truth is understandable and replay-driven.

##### What is deliberately NOT implemented yet

Manual outcome reconciliation workflow, message correction/edit, conversation export, or hidden internal diagnostics UI.

#### Substage 22.4: Complete native lifecycle, accessibility, and integration test coverage

##### Objective

Make the client robust enough for daily V0 use and ready for full product-path integration.

##### Why it happens now

All required screens/states/actions exist; platform lifecycle and quality gaps can be closed before real provider/deployment coupling.

##### Preconditions

Substages 22.1–22.3 pass with fixture backend.

##### Exact implementation work

- Handle app launch/terminate/background/foreground/network transition, window restoration, cache reset, revoked token, endpoint change, and backend upgrade/protocol mismatch.
- Add VoiceOver labels, keyboard navigation/focus, Dynamic Type/layout, contrast, selectable/copyable committed text, and reasonable transcript performance.
- Add unit reducer/store tests and UI tests for send/queue/draft/commit/failure/cancel/reconnect plus server fixture automation.
- Ensure tests can inject Keychain/clock/network/transport without exposing production token.
- Record app build/version/protocol in diagnostics safe for support.

##### Data/state introduced

Platform lifecycle/accessibility state and test artifacts.

##### Contracts/interfaces introduced

App lifecycle affects connections/cache only; backend work continues independently.

##### Failure behavior

Local cache/Keychain/network failures are presented as local configuration/connectivity, never backend work terminality.

##### Validation

Automated lifecycle/UI/accessibility suite, repeated force-quit/relaunch during pending/draft/running/completed states, memory/thread sanitizer where applicable, and localhost fixture run.

##### Exit criteria

- [ ] Native client survives normal lifecycle without duplicate work.
- [ ] Required states/actions are accessible.
- [ ] Client version/protocol diagnostics are safe.
- [ ] Full client test suite is stable.

##### What is deliberately NOT implemented yet

Notarized public distribution, crash analytics SDK, auto-update, App Store, mobile clients, or mature onboarding.

## Stage 23: Observability, evidence queries, and redaction closure

### Objective

Audit and complete instrumentation across every implemented subsystem so one work item can be quantitatively reconstructed without treating logs as canonical history.

### Why it happens now

Instrumentation has been added incrementally; all local/provider/client paths now exist, so missing correlations/measurements/redaction gaps can be found before integration/deployment gates.

### Preconditions

Stages 1–22 pass; representative deterministic and live-provider records exist.

### Exact implementation work

- Execute Substages 23.1–23.3.
- Add no public diagnostic/admin HTTP endpoint.
- Provide redacted offline inspection/evidence commands or query bundles for operators and acceptance automation.

### Data/state introduced

Complete structured spans/events, derived measurement queries/reports, redaction rules/tests, evidence manifests, and noncanonical operational inspection output.

### Contracts/interfaces introduced

Required span hierarchy/correlation fields, measurement definitions, content/secret classification, and `craxii-admin`-style read-only inspect/verify/evidence operations (plus the already needed device provision path).

### Failure behavior

Missing mandatory evidence or redaction violation fails verification/release. Telemetry sink loss does not alter journal truth but may make service unready if required diagnostics cannot be emitted safely.

### Validation

Trace/schema audits, work-to-event/row/artifact query reconciliation, sentinel secrets/content scans, latency/token/count arithmetic, high-cardinality label review, and evidence report regeneration after reopen.

### Exit criteria

- [ ] Every required question in the V0 goal can be answered from durable rows/artifacts plus traces.
- [ ] Span hierarchy and IDs correlate end to end.
- [ ] No secret/content/command/output leaks by default.
- [ ] Operational evidence is exportable without public admin surface.

### What is deliberately NOT implemented yet

Prometheus/OpenTelemetry backend, production SIEM, dashboards, automatic evaluations, content logging, or traces as recovery input.

### Substages

#### Substage 23.1: Complete span hierarchy and cross-layer correlation

##### Objective

Ensure every startup/request/work/context/model/tool/process/artifact/storage/replay/recovery/backup boundary has an owned structured span.

##### Why it happens now

All boundaries exist and can be checked against architecture's required list.

##### Preconditions

Stage 2 tracing base and real subsystem code exist.

##### Exact implementation work

- Instrument all required span names with service/build/runtime/subsystem plus request/device-pseudonym/Craxii/conversation/work/invocation/logical/tool/execution/workstation-generation/journal offset range as applicable.
- Make nesting reflect actual ownership: work contains selection/context/model/tool; process cleanup/artifact/journal transactions attach to owning attempt.
- Record state/reason/count/duration/status fields, not raw bodies/content/commands/env/output/headers/keys.
- Ensure spawned tasks preserve correlation explicitly and no detached span hides task failure.
- Use JSON in service and pretty local format with identical semantic fields.

##### Data/state introduced

Structured trace records in local/journald sinks.

##### Contracts/interfaces introduced

Tracing correlates behavior but never authorizes transition or fills missing durable evidence.

##### Failure behavior

Instrumentation failure cannot panic product paths casually; required initialization failure is fatal. Missing correlation in test is a verification failure.

##### Validation

Golden representative traces, parent/child/ID completeness assertions, async propagation tests, and source scan for prohibited raw fields.

##### Exit criteria

- [ ] Required span list is complete.
- [ ] One work ID traces every owned operation.
- [ ] IDs are trace fields, not low-cardinality metric labels.
- [ ] Content/secret classes are absent by default.

##### What is deliberately NOT implemented yet

Distributed tracing backend, sampling policy for scale, user-content debugging mode by default, or trace-based state repair.

#### Substage 23.2: Complete durable and derived measurements

##### Objective

Make work/model/context/tool/storage/recovery/protocol behavior quantitatively inspectable.

##### Why it happens now

All fields and events now have real producers and can be reconciled.

##### Preconditions

Representative successful/failing/cancelled/interrupted/replayed work exists.

##### Exact implementation work

- Verify/derive work queue/first progress/draft/answer/total/steps/invocations/attempts/tools/cancel/interruption/limit metrics.
- Verify model target/reason/config/hash/bytes/tokens/cache/reasoning/latencies/stop/calls/IDs/retry/draft/error and context source/contribution/utilization/estimator-error/growth metrics.
- Verify tool validation/authority/privilege/dispatch/start/duration/result/exit/signal/timeout/cancel/bytes/truncation/artifact/cleanup/unknown/generation metrics.
- Verify journal latency/busy/WAL/checkpoint/DB-artifact-disk/orphan/integrity/recovery and protocol command/dedup/conflict/replay/lag/slow/draft/auth/bootstrap metrics.
- Keep optional cost computation behind a separately versioned operator-supplied price table; store raw usage as truth.

##### Data/state introduced

Read-only SQL/report definitions and derived evidence output; optional versioned price input outside canonical inference evidence.

##### Contracts/interfaces introduced

Metric definitions name source fields/events and units; missing values are null/not observed, never zero guesses.

##### Failure behavior

Arithmetic/source mismatch or missing required field fails evidence validation. Metrics never cause product transitions.

##### Validation

Hand-calculate fixture metrics, reconcile event offsets/row times/monotonic durations, compare estimator/provider usage, and query every terminal class.

##### Exit criteria

- [ ] Required measurement lists are complete and unit-defined.
- [ ] Raw facts remain available beneath aggregates.
- [ ] Cost is versioned/optional, never guessed.

##### What is deliberately NOT implemented yet

Business KPI dashboards, SLO alerting platform, learned routing, retention analytics, or content quality scoring.

#### Substage 23.3: Implement safe operational inspection/evidence export and redaction tests

##### Objective

Let an operator answer “what happened?” and verify state without direct ad hoc mutation or public internals.

##### Why it happens now

Rows/traces/measurements are complete and final gates need reproducible evidence collection.

##### Preconditions

Substages 23.1–23.2 and State/Artifact integrity APIs exist.

##### Exact implementation work

- Implement a local/offline admin binary or subcommands for `preflight`, `verify-state`, `inspect-work`, `inspect-runtime`, and `evidence-export`, opening read-only where possible and using adapter APIs.
- Output redacted JSON/Markdown with IDs, state/events/context/target/reason/usage/tools/args hash or explicitly approved bounded args, privilege/duration/exit/truncation/artifacts/errors/recovery/replay—not secrets/raw output by default.
- Add artifact hash verification and journal/projection consistency results; link opaque artifact IDs/storage keys only for authorized local operator use.
- Build sentinel corpus for provider key, bearer token, auth header, env, user text, shell command, stdout/stderr, paths, and stack traces; scan traces/errors/protocol/evidence exports.
- Document journald/SQLite/read-only inspection commands and safe escalation for deeper content access.

##### Data/state introduced

Noncanonical evidence bundles and verification reports; no public state mutation.

##### Contracts/interfaces introduced

Operational inspection reads canonical sources but cannot redefine them; redaction policy is test-enforced across all outputs.

##### Failure behavior

Verification failure returns nonzero and preserves evidence. Permission/path/integrity errors are safe. Export refuses an unsafe content mode without explicit local opt-in.

##### Validation

Run commands on every fixture/terminal class, compare to direct test assertions, sentinel scan all sinks, verify read-only mode cannot update journal/projections.

##### Exit criteria

- [ ] Operators can inspect every required behavior quantitatively.
- [ ] Verification/export is deterministic and redacted.
- [ ] No public admin endpoint exists.
- [ ] Tools cannot access provider/client secrets through exported defaults.

##### What is deliberately NOT implemented yet

Remote admin console, live SQL mutation, automatic repair, public artifact browser, or centralized observability service.

## Stage 24: Full local deterministic integration gate

### Objective

Run the complete backend/SQLite/journal/scheduler/context/scripted model/tools/real Ubuntu workstation/protocol/replay/tracing path from clean state, including deterministic failures and restarts, as the first final integration gate.

### Why it happens now

All local product components and observability are complete. This gate proves architecture correctness before live provider/client/deployment variables are combined.

### Preconditions

Stages 1–23 pass; disposable Ubuntu 24.04 systemd/cgroup/sudo environment and test workspace are available.

### Exact implementation work

- Execute Substages 24.1–24.3.
- Use production composition except ScriptedProvider and explicitly disposable configuration/credentials.
- Archive a redacted evidence bundle and require zero process/file/DB leaks after cleanup.

### Data/state introduced

Disposable full-product database/WAL/artifacts/workspace, runtime/crash histories, protocol/client-simulator projections, traces, and a signed/checksummed test result manifest.

### Contracts/interfaces introduced

The “local deterministic release gate” becomes mandatory before real-provider, native, or deployment acceptance.

### Failure behavior

Any nondeterminism, skipped target-host test, mismatch, leak, missing evidence, false outcome, or retry violation blocks progression.

### Validation

Fresh/reopen/restart runs; happy/failure/duplicate/queued/cancel/context/replay/crash cases; projector/integrity/artifact/process checks; repeated evidence comparison.

### Exit criteria

- [ ] Complete deterministic product path passes on Ubuntu.
- [ ] All implemented failpoints and acceptance-like cases are repeatable.
- [ ] Required telemetry/evidence is complete and redacted.
- [ ] Cleanup returns environment to known state.

### What is deliberately NOT implemented yet

Real OpenAI nondeterminism, native UI integration, EC2/public TLS, backups, or release declaration.

### Substages

#### Substage 24.1: Build disposable full-system orchestration and fixtures

##### Objective

Create one reproducible command/runbook that provisions all local test dependencies without production authority.

##### Why it happens now

Subsystem tests need to become one controlled system test with attributable environment/version inputs.

##### Preconditions

Target Ubuntu packages/systemd/cgroup/sudo and deterministic fixture tooling exist.

##### Exact implementation work

- Create a disposable state/artifact/workspace root, config, random device token, scripted target/program, loopback server, and systemd-like service wrapper/cgroup.
- Record OS/kernel/arch/Bash/Git/Rust/binary/schema/protocol/config/toolset/provider-script versions and free disk.
- Seed only safe workspace fixtures/marker commands; ensure no production/customer credential/network route exists.
- Start backend, wait for ready, run via HTTP/WSS/client simulator, stop/kill/reopen, and clean after evidence capture.
- Make every nondeterministic ID/time normalized only in comparison output, never altered in actual DB.

##### Data/state introduced

Disposable environment manifest/config/token/state/artifacts/workspace/logs.

##### Contracts/interfaces introduced

Test orchestration uses public/operator interfaces, not private SQL setup except reviewed fixture seeding before service start.

##### Failure behavior

Prerequisite/capability mismatch reports “environment invalid” and does not count as pass. Cleanup failure preserves environment for diagnosis.

##### Validation

Create/run/destroy twice; verify permissions/no external authority/no residue and exact version manifest.

##### Exit criteria

- [ ] One-command/runbook environment is reproducible.
- [ ] Test and product paths are not bypassed.
- [ ] Environment provenance is recorded.

##### What is deliberately NOT implemented yet

Production provisioning, cloud snapshots, long-lived shared test database, or mock replacement for Ubuntu semantics.

#### Substage 24.2: Run deterministic happy, failure, concurrency, cancellation, and replay matrix

##### Objective

Prove all noncloud/nonclient product semantics together.

##### Why it happens now

The full disposable system is running with a deterministic provider.

##### Preconditions

Substage 24.1 ready state.

##### Exact implementation work

- Run machine inspection/follow-up restart, observed read/shell failures, malformed/unknown tool, provider transient/exhausted/failure-after-draft/refusal, multiple tools/mixed output, all limits.
- Run simultaneous exact duplicates/conflicts, later message during delayed tool, FIFO multiple work, queued/active cancellation, notification loss, replay/live race, slow client, app-like reconnect.
- Run full failpoint list implemented through Stage 18, including side-effect marker and graceful shutdown; verify no unintended automatic calls.
- Query and compare rows/events/manifests/artifacts/processes/traces/metrics after each case.

##### Data/state introduced

Complete deterministic case histories/evidence.

##### Contracts/interfaces introduced

The matrix maps each architecture acceptance/failure invariant to executable assertions before cloud/provider/client variants.

##### Failure behavior

One failed assertion fails the entire gate; cases reset isolated state so failures cannot contaminate later results.

##### Validation

Automated assertions plus independent final State Store/projector/artifact/process verification.

##### Exit criteria

- [ ] Every deterministic case passes.
- [ ] Queued causal isolation is source-ID proven.
- [ ] Unknown/cancel/replay semantics survive restart.
- [ ] All call/event/row counts are exact.

##### What is deliberately NOT implemented yet

Real model quality/wire failures, native views, AWS availability, or backup restore.

#### Substage 24.3: Approve deterministic gate evidence and freeze baseline

##### Objective

Turn the run into a reviewable go/no-go artifact for external integrations.

##### Why it happens now

Passing tests alone is insufficient if the system cannot explain how it passed.

##### Preconditions

Substage 24.2 passes with complete evidence.

##### Exact implementation work

- Generate an evidence index linking cases to config/build IDs, work/invocation/tool IDs, event cursor ranges, context manifests, artifact hashes, traces, process-cleanup and assertions.
- Run redaction scan, schema/protocol/fixture compatibility, dependency/security checks, and disk/WAL/checkpoint measurements.
- Review deviations/flakes; allow no semantic waiver. Fix and rerun any nondeterminism/missing evidence.
- Record baseline performance values without setting premature optimization targets.

##### Data/state introduced

Approved redacted deterministic evidence bundle and baseline metrics.

##### Contracts/interfaces introduced

External stages must preserve this semantic baseline and identify only expected adapter/environment differences.

##### Failure behavior

Missing/unredacted/unreproducible evidence invalidates the pass.

##### Validation

Have the evidence exporter reconstruct every required answer and rerun from bundle identifiers on preserved state.

##### Exit criteria

- [ ] Evidence is complete, redacted, reproducible, and reviewed.
- [ ] Baseline config/versions are frozen.
- [ ] Stage 25 may introduce real OpenAI.

##### What is deliberately NOT implemented yet

Release signoff, optimization commitments, customer-facing reports, or cloud/native acceptance.

## Stage 25: Real OpenAI headless integration gate

### Objective

Run the complete headless Craxii responsibility path against the current real OpenAI target and real Ubuntu tools, proving live model/tool continuation and provider evidence before a client or cloud deployment is involved.

### Why it happens now

Deterministic semantics are frozen and the adapter has passed a narrow smoke test. This isolates provider behavior from native/network/deployment variables.

### Preconditions

Stages 19 and 24 pass; development key/model/rate limits and safe Ubuntu environment are available.

### Exact implementation work

- Execute Substages 25.1–25.3.
- Use the same HTTP/State Store/agent/tool path as release; only the UI/public-cloud topology is absent.
- Preserve exact provider/current-doc verification evidence.

### Data/state introduced

Live model selection/context/invocation/tool/final records, provider IDs/usage/latencies/retries/continuations, real Ubuntu evidence, traces, and headless acceptance report.

### Contracts/interfaces introduced

The real-provider headless gate establishes that current model behavior can satisfy the canonical task through Craxii custom tools without provider conversation state.

### Failure behavior

Provider nondeterminism may require repeated user-level attempts with new work IDs, but infrastructure must never hide/retry semantic output or side effects. Account/model prerequisites are reported separately from architecture failures.

### Validation

Canonical task, follow-up after restart, observed tool failure, classified transient/fatal provider fixture or safe live failure, usage/latency/context/tool evidence, and provider-state independence.

### Exit criteria

- [ ] Real OpenAI drives at least one actual Ubuntu tool loop to correct final answer.
- [ ] Restart follow-up uses durable full context.
- [ ] Provider evidence and redaction are complete.
- [ ] Deterministic baseline remains green.

### What is deliberately NOT implemented yet

Native client, EC2/public TLS, production model budget, model quality tuning/routing, second provider, or final release benchmark.

### Substages

#### Substage 25.1: Freeze current live target and provider prerequisites

##### Objective

Make the live test reproducible and distinguish configuration/access failures from code/architecture failures.

##### Why it happens now

Current model IDs/capabilities/limits/API fields/rates are external facts and may have changed since adapter smoke.

##### Preconditions

OpenAI credential/project access and official docs are reachable.

##### Exact implementation work

- Reverify chosen model supports Responses, streaming, custom functions, ordered output needed, target context/output limits, stateless continuation option, and account rate limits.
- Record model ID/snapshot if available, target config version, estimator, prompt/toolset versions, API verification date, and test spending cap.
- Validate key through server-side secret source only; scan environment/child config/logs.
- Preflight network/DNS/TLS/time/free disk and safe Ubuntu tool facts; capture no production credentials.

##### Data/state introduced

Live-test configuration/provenance manifest and secret reference (raw key remains outside evidence).

##### Contracts/interfaces introduced

External values are runtime config, never source constants/domain identifiers.

##### Failure behavior

Unavailable/unauthorized/rate-limited model blocks live gate as a prerequisite with actionable classification; no fallback model is chosen silently.

##### Validation

Config/preflight command, official-doc link/date review, key sentinel scan, and one minimal no-tool health invocation if needed within budget.

##### Exit criteria

- [ ] One capable enabled target is recorded.
- [ ] Credential/network/account limits are usable.
- [ ] No secret can reach tools/logs/artifacts/context.

##### What is deliberately NOT implemented yet

Automatic current-model discovery/substitution, pricing optimization, production key, or multi-target routing.

#### Substage 25.2: Run canonical real-provider headless task and restart follow-up

##### Objective

Prove real inference chooses/uses Craxii tools and answers from observed/persisted state.

##### Why it happens now

Live target and full headless system are ready.

##### Preconditions

Substage 25.1 and clean test conversation/workspace exist.

##### Exact implementation work

- Submit the canonical inspection prompt through authenticated HTTP; allow model to choose one/several calls; require actual Ubuntu LocalWorkstation operations and correct observed answer.
- Verify selection reason, context manifest/source list/token estimate, pre-call intent, ordered output/tool args, pre-dispatch intent, privilege/cwd/output/exit/artifacts/cleanup, second invocation, final transaction, durable delivery.
- Kill backend after completion, reopen/new runtime/recovery/reconnect, ask Git-version follow-up, and require correct answer with provider conversation/previous response disabled.
- Record exact invocation/tool counts and provider usage/latencies/IDs without asserting a hard-coded call sequence.

##### Data/state introduced

Live canonical conversation and complete evidence.

##### Contracts/interfaces introduced

Model proposal flexibility is allowed; orchestration/evidence/correct machine facts are mandatory.

##### Failure behavior

Incorrect model answer may be retried only as a new explicit user work for quality diagnosis; infrastructure cannot rewrite facts or hide failed attempt. Any side-effect ambiguity follows normal policy.

##### Validation

Independent OS/Git commands, DB/event/context/artifact/trace queries, disabled continuation/provider-state run, and post-restart memory-empty assertion.

##### Exit criteria

- [ ] Canonical live headless path succeeds.
- [ ] Answer matches independently observed facts.
- [ ] Follow-up comes from reconstructed durable context.
- [ ] All provider/tool/context evidence is queryable.

##### What is deliberately NOT implemented yet

Native UI display, EC2 target benchmark, prompt/model tuning to guarantee one command shape, or production workload.

#### Substage 25.3: Validate live provider errors, retries, cancellation, and evidence

##### Objective

Confirm real adapter/Gateway behavior around provider conditions without compromising deterministic semantics.

##### Why it happens now

The happy live path works; failure/usage behavior must be inspected before combining with the client.

##### Preconditions

Substage 25.2 and local fixture-based error suite pass.

##### Exact implementation work

- Use safe configured invalid key/model/request cases and/or local proxy fault injection to produce auth, 429/Retry-After, 5xx/connect reset/no-output timeout, failure after draft, cancellation during request/backoff, and malformed/unknown event fixtures.
- Verify only classified pre-output transients retry up to bound with separate attempts; exposed draft/auth/invalid/context/cancel/unknown semantic cases do not.
- Confirm no tool dispatch until one complete successful response commits and provider ambiguity affects billing/output evidence only.
- Reconcile reported usage against estimator/context stats and preserve nullable/unavailable fields honestly.

##### Data/state introduced

Live/fault-injected provider attempt/error/retry/cancel evidence and estimator comparison report.

##### Contracts/interfaces introduced

Live error observations validate adapter mapping; deterministic fixture suite remains normative for hard-to-induce cases.

##### Failure behavior

Unsafe attempt to induce provider incidents is forbidden; use local fault proxy/fixtures where live action is not controlled. Any unclassified current wire error blocks release adapter readiness.

##### Validation

Inspect attempts/events/delays/draft flags/call counts/normalized errors/traces and secret/body redaction.

##### Exit criteria

- [ ] Retry/nonretry behavior matches policy.
- [ ] Cancellation stops local wait/backoff and no tool starts.
- [ ] Usage/latency/error evidence is complete.
- [ ] Unknown current semantics fail closed.

##### What is deliberately NOT implemented yet

Provider outage SLA, cross-provider fallback, automatic billing reconciliation, load/rate optimization, or final EC2 acceptance.

## Stage 26: Native macOS full-path integration gate

### Objective

Run the actual native app through authenticated HTTP, bootstrap, WSS replay/drafts, backend scheduling, scripted and real models, real Ubuntu tools, cancellation, reconnect, and committed reconciliation before cloud deployment.

### Why it happens now

The native app and both deterministic/real-provider headless systems are separately proven. This isolates client integration from AWS/TLS provisioning.

### Preconditions

Stages 22, 24, and 25 pass; the Mac can reach a safe local/disposable Ubuntu backend; device token is installed in Keychain.

### Exact implementation work

- Execute Substages 26.1–26.3.
- First use ScriptedProvider for deterministic UI/network assertions, then real OpenAI for the actual product path.
- Retain client/backend correlated evidence without recording the bearer token or raw sensitive content.

### Data/state introduced

Native app cache/outbox/cursor/Keychain state, complete backend conversation/work evidence, client screenshots/test logs, correlated connection/replay/draft metrics, and integration report.

### Contracts/interfaces introduced

The native integration gate proves the macOS app is a disposable but complete projection/control surface for one persistent Craxii.

### Failure behavior

Client/network loss cannot stop or fail backend work; app retries preserve IDs; projection uncertainty triggers bootstrap; real model/tool failures retain their backend certainty.

### Validation

Deterministic send/queue/draft/final/failure/duplicate/reconnect/cancel scenarios plus real OpenAI machine inspection/follow-up through the app.

### Exit criteria

- [ ] The native app completes real model/tool work through the actual backend path.
- [ ] Lost responses/sockets/app restarts do not duplicate or lose committed facts.
- [ ] Required terminal/unknown states render honestly.
- [ ] Client/server cursors and IDs correlate in evidence.

### What is deliberately NOT implemented yet

Public Internet/TLS deployment, distribution/notarization, production credentials, multi-device mature auth, or final release signoff.

### Substages

#### Substage 26.1: Run native client against deterministic full backend

##### Objective

Make every client-visible race/state deterministic through the real app and backend transports.

##### Why it happens now

Fixture-level client tests cannot prove end-to-end HTTP/WebSocket timing and app lifecycle behavior.

##### Preconditions

Disposable Ubuntu deterministic backend and debug Mac app configuration are ready.

##### Exact implementation work

- Provision the token into Keychain, bootstrap the real app, submit work, and observe pending/queued/running/tool/draft/committed states.
- Exercise simultaneous duplicate submission/lost HTTP response, second queued message during delayed tool, observed tool failure, outcome-unknown recovery fixture, queued/active cancellation, context limit, refusal/provider failure.
- Disconnect/reconnect socket, force-quit/relaunch app at pending/draft/after-commit points, and commit between bootstrap/WebSocket sync.
- Compare app reducer state/cursor with backend snapshot/journal and capture accessible screenshots for each terminal state.

##### Data/state introduced

Deterministic native/backend integration histories, app caches, and screenshot/evidence artifacts.

##### Contracts/interfaces introduced

Actual URLSession/WebSocket/Keychain/App lifecycle implementation must match fixture semantics exactly.

##### Failure behavior

Any app/server divergence, duplicate command, retained stale draft, missing committed event, or misleading terminal display blocks progression.

##### Validation

Automated UI integration where stable plus manual checks for visual/accessibility behavior; server evidence exporter and client cursor/state assertions.

##### Exit criteria

- [ ] Every client-visible deterministic case converges.
- [ ] App restart/reconnect uses same command/cursor truth.
- [ ] UI state matches backend certainty and order.

##### What is deliberately NOT implemented yet

Real provider quality, Internet latency/TLS, release signing, or backup restore.

#### Substage 26.2: Run real OpenAI machine inspection through native app

##### Objective

Prove the V0 end goal through the actual product surface before EC2 variables are introduced.

##### Why it happens now

Deterministic native integration and real-provider headless integration are green.

##### Preconditions

Substage 26.1, Stage 25 live target, and safe Ubuntu backend are ready.

##### Exact implementation work

- From the app, submit the exact canonical machine-inspection prompt with a stable client ID.
- Observe queued/model/tool/draft/committed UI and independently verify Ubuntu OS/architecture/cwd/Git answer.
- Kill/restart backend process, wait for recovery/readiness, reconnect app from last cursor, ask the Git-version follow-up, and verify durable-history answer.
- Inspect correlated client/server request/work/invocation/tool/runtime/cursor evidence and confirm provider conversation state is unused.

##### Data/state introduced

Real native product-path conversation/evidence.

##### Contracts/interfaces introduced

The app does not orchestrate model/tools; its only mutations remain HTTP message/cancel commands.

##### Failure behavior

Model quality failure is recorded separately from client/protocol failure. App disconnect cannot cause tool/model retry or false completion.

##### Validation

Backend evidence export, app transcript/cursor, independent machine facts, new runtime/recovery events, and Keychain/log redaction scan.

##### Exit criteria

- [ ] Canonical prompt and follow-up succeed through native app.
- [ ] Restart/reconnect is transparent but evidence-visible.
- [ ] Provider/client secrets remain isolated.

##### What is deliberately NOT implemented yet

Target EC2 hostname/Caddy/systemd/EBS, production workload, or final benchmark certification.

#### Substage 26.3: Approve native integration behavior and diagnostics

##### Objective

Freeze the app/backend compatibility baseline for deployment.

##### Why it happens now

Both deterministic and real native paths pass; cloud work should not alter protocol/UI semantics.

##### Preconditions

Substages 26.1–26.2 evidence is complete.

##### Exact implementation work

- Record app/backend/protocol/schema/build versions, endpoint namespace, test device ID pseudonym, cursor ranges, and all pass/fail cases.
- Run full native unit/UI/integration suite, backend deterministic suite, provider contract/live smoke, and redaction scan together.
- Document developer setup for endpoint/token import and safe cache reset; exclude raw token from report.
- Resolve all protocol fixture drift and UI state ambiguity before deployment.

##### Data/state introduced

Approved native compatibility/evidence baseline.

##### Contracts/interfaces introduced

Cloud deployment may change endpoint/TLS/latency only; public protocol and client state semantics are frozen for V0.

##### Failure behavior

Any waiver/missing correlation invalidates the gate; fix and rerun.

##### Validation

Independent review of evidence index and a clean-machine Mac setup/reconnect run.

##### Exit criteria

- [ ] App/backend compatibility is versioned and repeatable.
- [ ] Setup/reset diagnostics are safe.
- [ ] Stage 27 may provision target infrastructure.

##### What is deliberately NOT implemented yet

App distribution/updates, production support tooling, or protocol evolution.

## Stage 27: AWS/EC2 deployment foundation and release assets

### Objective

Create repeatable infrastructure and host/release configuration for one source-restricted x86-64 Ubuntu 24.04 EC2 workstation with encrypted root/data EBS, non-root backend, broad explicit admin path, systemd, Caddy, server-side credentials, and off-guest snapshot policy.

### Why it happens now

Local/native correctness is complete. Infrastructure can now reproduce known behavior rather than becoming a debugging environment for unfinished semantics.

### Preconditions

Stage 26 passes; AWS/DNS/source-CIDR/model/device/backup inputs are supplied; budget and no-production-authority policy are approved.

### Exact implementation work

- Execute Substages 27.1–27.5.
- Use one declarative Terraform stack under `ops/aws/terraform/` for V0 resources; do not maintain competing CloudFormation/manual definitions. Keep Terraform state outside Git in a protected operator backend.
- Build releases on x86-64 Ubuntu 24.04 and deploy immutable checksummed artifacts, never Cargo source execution on host.

### Data/state introduced

Terraform state/resources, VPC/subnet inputs, security group, Elastic IP/DNS mapping, EC2 instance, KMS-encrypted root/data EBS, DLM/AWS Backup policy, OS user/filesystem/mount/sudo/cgroup configuration, release/config/credential/systemd/Caddy assets.

### Contracts/interfaces introduced

Deployment input manifest, host filesystem/ownership contract, release manifest/preflight/atomic symlink/rollback contract, systemd service behavior, Caddy TLS proxy behavior, and backup ownership outside guest.

### Failure behavior

Provision/config/build/checksum/preflight/mount/permission/secret/unit/TLS failure leaves service unready/unexposed and preserves prior compatible release/state. No fallback public bind or unencrypted volume is allowed.

### Validation

Terraform plan/policy review, clean host provision rehearsal, mount/reboot/permissions, non-root/admin/cgroup tests, release checksum/revision/schema preflight, unit/Caddy config verification, and security/secret scans.

### Exit criteria

- [ ] Target infrastructure is reproducible and source-restricted.
- [ ] Separate encrypted persistent data volume and off-guest snapshot owner exist.
- [ ] Host supports all verified Workstation semantics.
- [ ] Release/config/secrets/services are immutable/restricted and rollback-aware.

### What is deliberately NOT implemented yet

Multi-AZ/HA, load balancer, autoscaling, Kubernetes, instance profile with customer authority, zero downtime, automated workstation replacement, or production credentials/data.

### Substages

#### Substage 27.1: Freeze deployment inputs, trust boundary, and infrastructure plan

##### Objective

Resolve all environment-specific values without leaking them into domain/source contracts.

##### Why it happens now

Terraform/host assets need exact inputs and an approved safety boundary.

##### Preconditions

AWS account/operator access, budget, region/AZ, DNS zone/hostname, client CIDR, and backup owner are known.

##### Exact implementation work

- Record AWS account/region/AZ, existing VPC/public subnet or explicitly created minimal network, hostname/Route53 zone, source CIDRs for 443 and optional SSH, x86-64 Ubuntu 24.04 AMI selection method, instance class at least 4 vCPU/16 GiB, storage sizes/types, KMS choice, snapshot retention, tags, and operator contacts.
- Decide SSH-based source-restricted initial administration (or explicitly equivalent) with no customer/production authority; instance profile absent by default.
- Define threat/failure boundary: one dev VM/root domain, outbound Internet allowed, no production routes/credentials, snapshot deletion unavailable to guest.
- Define Terraform state backend/access/locking/backup and ensure state/secrets are ignored from Git/evidence.
- Create a deployment input checklist blocking apply when hostname/CIDR/encryption/backup/no-production declarations are missing.

##### Data/state introduced

Versioned nonsecret deployment input manifest and protected Terraform backend metadata.

##### Contracts/interfaces introduced

Cloud IDs are deployment evidence only; configured stable Craxii/workstation IDs come from/restored into SQLite.

##### Failure behavior

Missing/unsafe/wildcard source range, ARM AMI, undersized host, unencrypted/delete-on-termination data volume, or authority exposure fails preflight.

##### Validation

Peer review input manifest/threat boundary, AWS permissions, CIDRs, AMI architecture/release, quotas/cost estimate, and Terraform backend security.

##### Exit criteria

- [ ] Every deployment prerequisite has a value/owner.
- [ ] No customer/production authority is reachable.
- [ ] Backup control is external to guest.
- [ ] Cloud values remain out of domain identity.

##### What is deliberately NOT implemented yet

Apply/provision, private connectivity/VPN, SSM requirement, multiple workspaces/VMs, production networking, or disaster recovery automation.

#### Substage 27.2: Implement declarative EC2, network, EBS, KMS, DNS, and snapshot resources

##### Objective

Create the physical target and recovery plane repeatably.

##### Why it happens now

Inputs/trust rules are frozen and can be encoded without application ambiguity.

##### Preconditions

Substage 27.1 and Terraform tooling/operator credentials are available.

##### Exact implementation work

- Define security group ingress 443 from current client CIDR and optional 22 from admin CIDR only; restrict backend listener by host config; allow necessary egress.
- Define x86-64 Ubuntu 24.04 instance, stable Elastic IP if used for DNS continuity, encrypted replaceable root EBS, and encrypted data EBS with `DeleteOnTermination=false` and required tags.
- Use customer-managed KMS or approved encrypted-volume key policy with operator/recovery access and no guest deletion authority.
- Create DNS record for hostname/EIP and DLM/AWS Backup daily snapshot policy retaining at least 14 points plus on-demand release snapshots; service role lives in AWS control plane, not instance.
- Output only nonsecret deployment facts and record resolved AMI/instance/volume/KMS/security/DNS policy IDs in deployment evidence.

##### Data/state introduced

AWS resources and Terraform state.

##### Contracts/interfaces introduced

Data volume persists beyond instance termination; snapshots are recovery copies; instance identity never equals Craxii identity.

##### Failure behavior

Terraform partial apply is reconciled through plan/state, not ad hoc duplicate resources. Encryption/retention/ingress drift blocks deployment.

##### Validation

Plan/apply in disposable environment, AWS describe/policy checks for architecture/AMI/encryption/delete flag/SG/profile/snapshot schedule/DNS, terminate test instance while retaining data volume where safe.

##### Exit criteria

- [ ] Required resources exist with exact safety attributes.
- [ ] Data volume and snapshots survive guest/instance lifecycle.
- [ ] Public ingress is source-restricted.
- [ ] No broad instance role exists.

##### What is deliberately NOT implemented yet

Multi-region snapshot copy, autoscaling, load balancer/WAF, private DNS/VPN, or active-active state.

#### Substage 27.3: Provision Ubuntu user, data layout, tooling, sudo, and cgroup support

##### Objective

Make the EC2 host behaviorally equivalent to the proven Ubuntu LocalWorkstation environment.

##### Why it happens now

Physical resources exist; application deployment depends on correct mount/ownership/admin/process semantics.

##### Preconditions

Substage 27.2 instance/data volume and source-restricted admin access exist.

##### Exact implementation work

- Apply security updates; create nonlogin-or-login-as-needed `craxii` user/group with `/home/craxii`; install Bash/Git/CA/SQLite operational tools/Caddy and required compiler/Docker/dev packages at recorded versions or bootstrap policy.
- Format/mount data EBS by filesystem UUID; create one data-root filesystem with durable subdirectories and bind mounts to `/var/lib/craxii`, `/srv/craxii/workspaces`, and selected durable `/home/craxii` state; ensure same-filesystem artifact tmp/final rename and fstab reboot ordering.
- Create `/opt/craxii/releases`, `/opt/craxii/current`, `/etc/craxii`, credentials, caches, and `/run/craxii` with exact root/craxii owners/modes/UMask expectations.
- Install explicit `sudo -n` policy permitting broad development-workstation administration while backend remains user; do not use `NoNewPrivileges` or misleading filesystem sandbox; keep Docker invocation privilege explicit.
- Verify unified cgroup v2 and systemd `Delegate=yes` support for service subtree; configure process/file limits without breaking intended engineering use.

##### Data/state introduced

OS packages/user/groups/sudoers/filesystems/mounts/directories/permissions/cgroup capabilities and durable development workspace state.

##### Contracts/interfaces introduced

Filesystem paths/ownership match architecture; operational separation is hygiene, not root security containment.

##### Failure behavior

Mount/permission/sudo/cgroup/tool mismatch blocks application readiness. Never fall back to root backend, root-volume canonical state, or untracked process cleanup.

##### Validation

Reboot/mount checks, owner/mode/fstab same-filesystem rename, non-root service/admin child/Docker/package/systemctl tests, cgroup descendant cleanup, environment/secret scans, OS/arch/Git capabilities.

##### Exit criteria

- [ ] All canonical/workspace paths persist on data EBS as designed.
- [ ] Backend user and admin path behave exactly as LocalWorkstation tests require.
- [ ] Reboot preserves mounts/permissions/cgroups.
- [ ] No production authority is present.

##### What is deliberately NOT implemented yet

Immutable OS images, automatic reprovision/reattach, per-project users/VMs, strong sandboxing, or workstation credential broker.

#### Substage 27.4: Build immutable Linux release, preflight, deployment, and rollback assets

##### Objective

Produce and install a reproducible release binary/config bundle without compiling on the workstation at service start.

##### Why it happens now

The host layout is ready and local/native gates define the artifact behavior.

##### Preconditions

Clean x86-64 Ubuntu 24.04 builder, pinned Rust/Cargo.lock, source revision, and host admin access exist.

##### Exact implementation work

- Build/test release on x86-64 Linux; record semantic version, Git revision, target/toolchain, Cargo.lock hash, binary checksum/size, protocol/schema/architecture versions, and dependency/advisory results.
- Package `craxii-server` and local admin binary plus versioned nonsecret config/systemd/Caddy templates/migration metadata; exclude secrets/databases/workspaces/build cache.
- Implement preflight that validates config/credential references/paths/permissions/OS/arch/cgroups/sudo/default model/schema compatibility without external side effects or migrations unless explicitly selected.
- Upload to root-owned immutable `/opt/craxii/releases/<version>`, verify checksum, and atomically switch `current` only after preflight/pre-migration snapshot policy.
- Retain at least one previous schema-compatible release and implement explicit rollback compatibility check before symlink switch/restart.

##### Data/state introduced

Release artifacts/manifests/checksums and immutable host release directories/symlink.

##### Contracts/interfaces introduced

systemd executes a versioned release binary; a release declares min/max schema/protocol compatibility; rollback never opens unsupported schema.

##### Failure behavior

Build/test/checksum/preflight/compatibility failure leaves current service/release unchanged. No automatic downgrade migration.

##### Validation

Rebuild checksum/reproducibility to practical degree, tamper checksum, invalid config/schema/host preflight, atomic symlink, restart previous compatible binary, and read-only artifact permissions.

##### Exit criteria

- [ ] Release is traceable to exact source/dependencies/target.
- [ ] Host never runs Cargo as service.
- [ ] Deployment switch is atomic and reversible when schema-compatible.
- [ ] Preflight has no workstation/provider side effect.

##### What is deliberately NOT implemented yet

CI/CD auto-deploy, zero downtime, containerized backend, destructive down migrations, or unattended rollback.

#### Substage 27.5: Implement systemd, Caddy, typed config, credentials, and service ordering assets

##### Objective

Supervise the backend/proxy with correct privilege, cgroup cleanup, startup ordering, TLS forwarding, secrets, and restart bounds.

##### Why it happens now

Host/release paths and runtime requirements are fixed.

##### Preconditions

Substages 27.3–27.4 and DNS hostname exist.

##### Exact implementation work

- Create `craxii.service` with non-root user/group, release ExecStart/config, `/var/lib/craxii` working dir, on-failure restart/2s delay, rate limits, TERM, control-group kill, 30s stop, 0077 umask, `Delegate=yes`, mount/network ordering, file/process limits, systemd credentials.
- Do not set flags that block workstation ownership/admin behavior (`NoNewPrivileges`, `ProtectHome`, restrictive filesystem sandbox) without verified equivalent.
- Place nonsecret typed config root-owned/readable as needed; provide OpenAI key through systemd credential or root-owned credential source into adapter only; provision device hash through offline admin command and import raw token only to Mac Keychain.
- Create minimal Caddyfile for hostname: 443 trusted cert/renewal, loopback reverse proxy/WS upgrade, forwarded headers, compatible body/idle limits, no auth/body/WebSocket payload logging, no DB/workspace/key access.
- Configure backend loopback bind and trust forwarding headers only from loopback; expose minimal liveness and restrict readiness detail.

##### Data/state introduced

Unit/Caddy/config/credential/device rows/files and journald service logs.

##### Contracts/interfaces introduced

systemd restarts processes/kills ordinary descendants; startup recovery restores semantics. Caddy owns TLS only; backend owns auth/protocol/state.

##### Failure behavior

Deterministic config/schema failure is rate-limited, unready, and operator-visible without infinite loop. Missing credential/cert/mount prevents usable service; no public backend bind fallback.

##### Validation

`systemd-analyze verify`, `caddy validate`, unit start/stop/TERM/KILL/restart/rate limit, descendant cleanup, credential/env/FD/log scan, loopback bind/forwarded-header tests, certificate staging where possible.

##### Exit criteria

- [ ] Units/config pass syntax/behavior checks.
- [ ] Provider credential is server-side and omitted from child env/logs.
- [ ] Backend is loopback/non-root; Caddy owns only edge transport.
- [ ] Service cgroup cleanup/restart bounds are configured.

##### What is deliberately NOT implemented yet

Live production hostname issuance/benchmark, load balancer, public admin API, cert handling in Rust, or mature secret/authority service.

## Stage 28: EC2 deployment, HTTPS/WSS live operation, and target integration

### Objective

Apply the deployment foundation, start Craxii on the real persistent EC2 workstation, connect the native Mac over source-restricted HTTPS/WSS, and verify target machine/admin/systemd/reconnect behavior.

### Why it happens now

Infrastructure/release assets and all local product paths are proven. This assembles the required physical topology before backup and final benchmark verification.

### Preconditions

Stage 27 assets review cleanly; AWS/DNS/OpenAI/device inputs are available; native app accepts the production-like endpoint.

### Exact implementation work

- Execute Substages 28.1–28.4.
- Deploy first to a clean V0 development EC2 instance with no production/customer authority.
- Preserve deployment evidence/config fingerprints and never include raw credentials.

### Data/state introduced

Live AWS host/volumes/snapshots/DNS/certificates, installed release/config/device/provider credential, canonical SQLite/artifacts/workspace on data EBS, journald/Caddy logs, and remote Mac client state.

### Contracts/interfaces introduced

The complete target topology—Mac HTTPS/WSS Caddy loopback Axum/systemd LocalWorkstation EBS/OpenAI—is operational with health/readiness/restart procedures.

### Failure behavior

Failed provision/migration/recovery/readiness/TLS/auth/tool smoke leaves endpoint unavailable/unready and retains evidence/prior compatible release. No relaxation of SG/TLS/auth to “make it work.”

### Validation

AWS/resource/host checks, release preflight/deploy/startup order, HTTPS/WSS certificate/auth/source restriction, real tool/admin/cgroup tests, remote app send/replay/cancel, SIGKILL/systemd restart, data-volume persistence.

### Exit criteria

- [ ] Real EC2 topology is live and source-restricted over trusted HTTPS/WSS.
- [ ] Backend is ready only after integrity/recovery/scheduler/model/tool checks.
- [ ] Native Mac can perform real Ubuntu/OpenAI work remotely.
- [ ] systemd restart and data persistence behave as designed.

### What is deliberately NOT implemented yet

Release acceptance signoff, restore proof, public broad access, HA, production authority, or automatic instance replacement.

### Substages

#### Substage 28.1: Provision, mount, configure, and deploy the first target instance

##### Objective

Bring a clean EC2 host from Terraform outputs to an honestly ready Craxii service.

##### Why it happens now

All declarative/host/release assets are available and validated in rehearsal.

##### Preconditions

Terraform apply authority, release artifact, config inputs, credentials, DNS, and source-restricted admin path exist.

##### Exact implementation work

- Apply/verify AWS resources; provision/mount/bind data volume; configure user/packages/sudo/cgroups/directories.
- Install immutable release/config/units/Caddy; place OpenAI systemd credential; provision device hash and securely transfer raw token once to Mac Keychain.
- Take on-demand pre-migration snapshot if applicable; run preflight, migrations, initial journal-aware bootstrap/new runtime/recovery; start scheduler/readiness then Caddy exposure.
- Record resolved AMI/instance/volume/KMS/snapshot/release/config/schema/runtime/workstation generation facts.

##### Data/state introduced

First live canonical V0 database/principal/conversation/workstation/workspace/device/runtime state and host deployment state.

##### Contracts/interfaces introduced

Startup order is operationally enforced; device and provider credentials are provisioned out of band and separately.

##### Failure behavior

Any failed step stops before next exposure; preserve volume/release/logs, do not rerun bootstrap with new IDs, and rollback binary only if schema-compatible.

##### Validation

Fresh startup/readiness, DB pragmas/integrity/projector, paths/mounts/permissions, runtime/build metadata, registry/target/capabilities, credential redaction, and reboot.

##### Exit criteria

- [ ] One stable Craxii identity exists on persistent data EBS.
- [ ] Service becomes ready in normative order.
- [ ] Reboot preserves state/mount/readiness.
- [ ] Deployment inventory is recorded.

##### What is deliberately NOT implemented yet

Restored identity from backup, migration of prior real history, second active server, or automatic reattach.

#### Substage 28.2: Enable and verify HTTPS/WSS edge, authentication, and source restrictions

##### Objective

Expose only the authenticated V0 protocol through a trusted certificate and narrow network boundary.

##### Why it happens now

Backend is healthy/loopback and DNS/Caddy resources are configured.

##### Preconditions

Substage 28.1, DNS propagation, source CIDR, and Caddy ACME ability exist.

##### Exact implementation work

- Obtain/renew trusted certificate for hostname; verify chain/name/time/TLS and Caddy persistence.
- Confirm security group permits 443 only from configured source and optional SSH only from admin source; backend port is unreachable externally.
- Test HTTPS message/bootstrap/cancel and WSS upgrade/replay/drafts through Caddy with bearer auth, request/body/idle limits, trusted forwarded headers, and no sensitive proxy logs.
- Test invalid/revoked/missing token, outside-source reachability, WebSocket long idle/reconnect, certificate renewal/staging path, Caddy restart independent of backend.

##### Data/state introduced

Caddy certificate/account state and edge connection/log evidence.

##### Contracts/interfaces introduced

Mac trusts normal TLS hostname; Caddy does not authenticate/interpret Craxii; backend accepts forwarding only from loopback.

##### Failure behavior

Certificate/proxy/auth/source mismatch keeps client disconnected; no HTTP downgrade/direct backend/public exception.

##### Validation

TLS scanner/client verification, AWS SG inspection, external port scan from allowed/disallowed sources where authorized, auth matrix, WS upgrade/replay, log/header/body scan.

##### Exit criteria

- [ ] Trusted HTTPS/WSS works from allowed Mac network.
- [ ] Unauthorized/outside-source/direct-backend access fails.
- [ ] Proxy logs/config expose no secrets/content.
- [ ] WebSocket survives configured idle/proxy behavior.

##### What is deliberately NOT implemented yet

Global public access, VPN/private gateway, load balancer/WAF, client certificates, consumer auth, or multi-device policy.

#### Substage 28.3: Verify target LocalWorkstation, privilege, process cleanup, and persistence

##### Objective

Confirm the EC2 host—not merely a lab Ubuntu VM—satisfies every workstation capability used by Craxii.

##### Why it happens now

The deployed service/edge is reachable; target-specific kernel/systemd/sudo/storage behavior must be proven before product use.

##### Preconditions

Substages 28.1–28.2 and disposable target workspace exist.

##### Exact implementation work

- Run capability/read-file/shell test subset through Tool Service/agent path, including OS/arch/cwd/Git, stdout/stderr cap, nonzero/signal/timeout, user/admin UID, package/Docker/temp service operations.
- Run child/grandchild/background cleanup and cancellation; inspect per-execution/service cgroups and no survivors.
- Kill backend during marker command; verify systemd service cgroup cleanup, restart recovery outcome-unknown/interrupted, no repeat.
- Reboot instance and verify data-volume SQLite/artifacts/workspaces/home state; root volume remains conceptually replaceable.
- Measure disk/free space/WAL/checkpoint/process limits and compare capabilities to config/DB.

##### Data/state introduced

Target tool/crash/recovery evidence and disposable workspace/package/container/service changes.

##### Contracts/interfaces introduced

Advertised target capabilities are backed by exact EC2 observations; service-level cleanup and semantic uncertainty remain separate.

##### Failure behavior

Any leaked process, false privilege, missing admin capability, secret inheritance, mount loss, or auto-repeat blocks target readiness/release.

##### Validation

State/trace/artifact/cgroup/process/mount/package/Docker/systemd queries and cleanup verification after each test/reboot.

##### Exit criteria

- [ ] Target capabilities equal tested behavior.
- [ ] Admin engineering operations are possible and recorded.
- [ ] Kill/restart cleanup and unknown semantics hold.
- [ ] Durable data survives reboot.

##### What is deliberately NOT implemented yet

Hostile root containment, durable daemons/process sessions, production package/service changes, or multi-workstation routing.

#### Substage 28.4: Connect the real Mac and complete remote target smoke path

##### Objective

Prove the actual geographically/network-separated product topology before formal benchmarks.

##### Why it happens now

Edge and target Workstation are independently valid.

##### Preconditions

Mac has hostname/token in Keychain and allowed source IP; backend/OpenAI ready.

##### Exact implementation work

- Bootstrap native app over HTTPS, open WSS, submit a safe real model/tool request, observe queue/draft/tool/final, and verify target—not Mac—facts.
- Disconnect network/app, allow backend work to continue, reconnect/replay; cancel a long safe command remotely and verify cleanup/UI.
- Restart Caddy and backend separately; verify proxy-only failure versus recovery/new runtime behavior and cursor convergence.
- Record end-to-end command ack, first progress/draft/final, provider/tool/replay latency and correlated IDs/cursors.

##### Data/state introduced

Remote native/EC2 product-path conversation and network/latency evidence.

##### Contracts/interfaces introduced

Network/client/proxy are replaceable delivery components; persistent work/state remains server-side.

##### Failure behavior

Network/Caddy/client loss cannot cancel/duplicate work. Backend death invokes systemd/recovery semantics. Auth/source change is explicit connectivity failure.

##### Validation

Compare app/server projection/cursors, independently query EC2 facts, inspect traces/rows/artifacts and connection metrics, scan logs/Keychain paths.

##### Exit criteria

- [ ] Real Mac→HTTPS/WSS→EC2→OpenAI/tools path works.
- [ ] Disconnect/reconnect/cancel/restart behavior is honest.
- [ ] End-to-end instrumentation correlates.

##### What is deliberately NOT implemented yet

Formal canonical/release benchmark evidence, backup restore, broad users, or production workloads.

## Stage 29: Backup, restore, deployment rollback, and operational runbooks

### Objective

Implement SQLite-consistent backups, off-guest snapshot coordination, restore-to-replacement-host procedure, schema-compatible deployment/rollback, disk/WAL/health inspection, and operator incident runbooks before verification.

### Why it happens now

The real target contains meaningful canonical/workspace state. Recovery cannot remain a paper promise before final crash/restore/acceptance gates.

### Preconditions

Stage 28 target is healthy; snapshot policy/resources and compatible prior release are available; a disposable restore instance budget is approved.

### Exact implementation work

- Execute Substages 29.1–29.4.
- Keep backup authority outside guest and treat backups as inactive recovery copies.
- Rehearse commands on disposable state before touching benchmark history.

### Data/state introduced

SQLite backup databases/manifests/hashes/heads, staging files, EBS snapshot catalog/ages, restore/deployment/rollback/preflight runbooks, operational measurements, and incident evidence templates.

### Contracts/interfaces introduced

Online backup operation, snapshot-confirmation/staging cleanup, explicit restore authority decision, release/schema compatibility matrix, and operator inspection/readiness/rollback procedures.

### Failure behavior

Backup/quick-check/hash/snapshot confirmation failure never deletes prior recovery copy. Restore never overwrites live state. Incompatible rollback is refused. Disk/integrity risk marks unready/stops claims.

### Validation

Consistent backup under writes/WAL, quick/hash/head checks, snapshot catalog/retention, isolated restore dry run, artifact/workspace checks, rollback rehearsals, disk-full/busy/checkpoint/credential/unit incident drills.

### Exit criteria

- [ ] Online backup includes committed WAL state and verifies independently.
- [ ] Off-guest snapshot policy/age is observable and guest cannot delete it.
- [ ] Restore/rollback runbooks are executable and schema-safe.
- [ ] Operators can diagnose/start/stop/recover without guessing.

### What is deliberately NOT implemented yet

Zero-RPO, continuous replication, automated failover, cross-region DR, workstation-independent live canonical state, or self-healing projection repair.

### Substages

#### Substage 29.1: Implement SQLite online backup and verification operation

##### Objective

Create a consistent destination database that includes committed WAL state without raw-copy hazards.

##### Why it happens now

The live database has canonical history and final restore verification needs a trusted copy primitive.

##### Preconditions

SQLite backup API/dependency choice is reviewed; secure backup staging on data volume exists.

##### Exact implementation work

- Add an operator-only backup command using SQLite online backup mechanism/vetted SQLx-compatible low-level API, not copying only `.sqlite3`.
- Create unique restrictive staging destination, record source DB identity/schema/build/current journal head/start/end, copy while normal short writes may continue, and capture the destination's actual included head.
- Run destination `quick_check`, foreign-key/application/projector consistency, compute size/SHA-256, and write a redacted manifest atomically.
- Instrument duration/pages/retries/WAL/checkpoint/destination head/hash/bytes.
- Retain staging until an off-guest snapshot containing it is confirmed; delete old staging only through explicit policy.

##### Data/state introduced

Consistent backup DB and manifest in `/var/lib/craxii/backups`, backup trace/measurements.

##### Contracts/interfaces introduced

Backup head identifies its recovery point; backup is read-only recovery copy, never active authority.

##### Failure behavior

Copy/verify/hash/disk failure produces no valid manifest and retains diagnostic partial safely; product DB continues or service degrades only if disk/integrity risk demands it.

##### Validation

Concurrent command/tool history writes during backup, reopen destination without WAL dependency, compare included head/projector/artifacts, corruption/low-disk/interrupt tests.

##### Exit criteria

- [ ] Destination is independently consistent/reopenable.
- [ ] Manifest accurately identifies source/included head/hash/schema.
- [ ] Raw main-file copy is nowhere used.

##### What is deliberately NOT implemented yet

Remote object upload, incremental backup, continuous replication, automatic restore, or zero data loss.

#### Substage 29.2: Coordinate off-guest snapshots, retention, and backup observability

##### Objective

Ensure recovery copies exist beyond guest authority and their recency/contents are known.

##### Why it happens now

Verified backup staging can now be captured by the AWS snapshot plane.

##### Preconditions

Substage 29.1 and DLM/AWS Backup policy are active.

##### Exact implementation work

- Run/confirm daily data-volume snapshots with at least 14 retention points and on-demand pre-migration/pre-release snapshots.
- Before important release, create verified SQLite backup then trigger/wait for snapshot containing its staging path; record snapshot ID/time/volume/config/release/backup head/hash in operator evidence.
- Verify instance role/guest credentials cannot list/delete snapshots beyond intentionally public metadata; snapshot service role is off-guest.
- Measure latest snapshot/verified backup age and alert or mark operational warning when stale; track disk usage for DB/WAL/artifacts/workspaces/Docker/caches/free space.
- Remove staging only after explicit snapshot confirmation and retention policy.

##### Data/state introduced

Off-guest EBS snapshots and snapshot/backup catalog/age/disk measurements.

##### Contracts/interfaces introduced

Snapshot is crash-consistent recovery layer; verified SQLite backup inside it provides preferred DB restore point; neither is active state.

##### Failure behavior

Snapshot/confirmation/retention failure keeps staging/prior snapshots and blocks release readiness; it does not grant guest broader AWS authority.

##### Validation

AWS API/policy/catalog checks, simulated stale/missing snapshot, staging-to-snapshot correlation, guest permission denial, and retention observation over test points.

##### Exit criteria

- [ ] Automated/off-guest recovery points exist and are current.
- [ ] Important snapshots reference verified DB backup evidence.
- [ ] Guest cannot delete recovery copies.
- [ ] Backup/disk age/size is inspectable.

##### What is deliberately NOT implemented yet

Cross-account/cross-region snapshots, immutable vault lock, zero-RPO replication, or automatic capacity scaling.

#### Substage 29.3: Implement isolated restore and workstation-generation procedure

##### Objective

Define the exact safe path for turning one recovery copy into a replacement test Craxii without corrupting live authority.

##### Why it happens now

Backups/snapshots exist; the final verification must execute a designed procedure, not improvise.

##### Preconditions

Compatible release, disposable test instance/network/no-production authority, and snapshot/backup catalog exist.

##### Exact implementation work

- Restore snapshot to a new volume/instance isolated from live hostname/device/provider credentials initially; mount read-only first and verify backup manifest/hash/DB integrity/artifacts.
- Copy/activate selected restored DB/artifacts/workspace into test paths; retain same `craxii_id`, conversation/work IDs, and logical workspace while assigning/incrementing workstation generation through explicit restore operation/evidence.
- Configure compatible binary/schema, new runtime, new local host evidence, and separately provision a test device/provider credential only after integrity passes.
- Run startup recovery/readiness, reconstruct benchmark conversation, inspect canonical artifacts/workspace state, and answer follow-up with ScriptedProvider then optional dev OpenAI.
- Record recovery point, data gap, recovery time, generation mapping, resource IDs, and teardown; never make restored copy simultaneously authoritative at live endpoint.

##### Data/state introduced

Replacement test VM/volume, restored canonical/workspace copy, new workstation generation/runtime/device test state, and restore report.

##### Contracts/interfaces introduced

Restore authority is explicit; identity meanings survive physical resource change; two active authorities are prohibited.

##### Failure behavior

Integrity/hash/schema/generation mismatch aborts before ready. Restore never edits/deletes live state/snapshot and never guesses missing commits.

##### Validation

Dry-run full procedure, compare source backup head/IDs/artifact hashes, recovery follow-up, network/credential isolation, and clean teardown while retaining report.

##### Exit criteria

- [ ] Restore procedure is complete and safe to execute.
- [ ] Same Craxii/history survives new host/generation.
- [ ] RPO/RTO/data-gap semantics are explicit.
- [ ] Live authority is never duplicated accidentally.

##### What is deliberately NOT implemented yet

Automatic failover/reattach, cryptographic external Craxii identity, selective clean-OS project sanitization after compromise, or mature RemoteWorkstation.

#### Substage 29.4: Complete deployment, rollback, integrity, and incident runbooks

##### Objective

Make normal and failed operations repeatable without unsafe database/file/process guesses.

##### Why it happens now

Release, service, backup, and restore mechanisms are all defined and can be combined into operator workflows.

##### Preconditions

Substages 29.1–29.3 and Stage 27 release assets exist.

##### Exact implementation work

- Document/automate deploy: verify deterministic/native/provider gates, build/checksum, pre-migration backup/snapshot, upload/preflight, compatibility, atomic symlink, restart, readiness/recovery/smoke, retain prior.
- Document rollback: stop/unready, verify previous binary supports current schema, switch/restart, or restore pre-migration copy to isolated/replacement host when incompatible; never down-migrate ad hoc.
- Document incidents for config/schema failure loop, integrity mismatch, disk full, WAL growth/checkpoint pin, `SQLITE_BUSY`, missing artifact, process leak/cgroup failure, provider auth/outage, Caddy/cert failure, token revoke, snapshot staleness.
- Provide safe local commands for service/journald/state verification, backup/snapshot catalog, process/cgroup/mount/disk, and redacted work evidence.
- Define operator decision points/owners and conditions to stop claims/mark unready/quarantine host.

##### Data/state introduced

Versioned operational runbooks/checklists and incident evidence templates.

##### Contracts/interfaces introduced

Operational changes preserve schema compatibility/canonical state and leave an audit trail separate from model context.

##### Failure behavior

Ambiguous/incompatible condition stops and escalates; no destructive reset/delete/rebuild shortcut is authorized by a runbook.

##### Validation

Tabletop plus disposable live drills for deploy/compatible rollback/config failure/disk/WAL/process/provider/Caddy/token/snapshot incidents; verify commands and evidence.

##### Exit criteria

- [ ] Deploy/rollback/incident procedures are executable and reviewed.
- [ ] Incompatible rollback routes through backup/restore, not guessing.
- [ ] Operators know when to stop claims/quarantine/escalate.

##### What is deliberately NOT implemented yet

Automatic remediation, paging platform, blue/green deployment, live state mutation console, or production on-call program.

## Stage 30: Observability and evidence verification gate

### Objective

Prove that the running target exposes enough redacted, correlated evidence to explain every work, model, context, tool, replay, restart, persistence, and recovery decision quantitatively without treating telemetry as canonical history.

### Why it happens now

All runtime, client, provider, deployment, and recovery paths now exist. Verification before fault testing prevents missing evidence from making a later crash result uninterpretable.

### Preconditions

Stages 23 and 28–29 pass; a target test conversation and access to redacted logs, SQLite inspection tools, artifacts, systemd, and Caddy metrics exist.

### Exact implementation work

- Execute Substages 30.1–30.3.
- Freeze the required-field/evidence matrix and the exact inspection queries/commands used by acceptance.
- Run verification on both normal and deliberately failing work and preserve a redacted evidence bundle.

### Data/state introduced

Observability verification report, correlation reconstruction, quantitative baseline, redaction scan, operational inspection transcript, and evidence-bundle manifest.

### Contracts/interfaces introduced

The evidence bundle is a derived diagnostic artifact keyed by durable IDs/cursors and build/config metadata; it is neither an event source nor eligible model context.

### Failure behavior

Missing correlation, unredacted secrets/content, contradictory journal/projection/log data, or an unmeasured required dimension fails the release gate. Telemetry loss never fabricates canonical state.

### Validation

Reconstruct representative work using only approved inspection surfaces, reconcile it to canonical rows/events, verify all required metrics and redaction canaries, and repeat across restart/reconnect.

### Exit criteria

- [ ] Every required behavioral question has a reproducible evidence source.
- [ ] Correlation and quantitative fields reconcile to canonical state.
- [ ] Secret/content redaction and telemetry/canonical separation pass.
- [ ] Operators can inspect the deployed target without ad hoc database mutation.

### What is deliberately NOT implemented yet

Distributed observability SaaS, production alerting/on-call, arbitrary user analytics, model-content indexing, or tracing as recovery state.

### Substages

#### Substage 30.1: Verify end-to-end correlation and evidence completeness

##### Objective

Demonstrate that one work item's complete causal chain can be reconstructed from acceptance through final delivery.

##### Why it happens now

The complete target topology and all durable record types are available.

##### Preconditions

Representative successful, observed-failure, cancelled, interrupted, duplicate, and reconnected works exist.

##### Exact implementation work

- Define a field matrix for `craxii_id`, conversation/message/work/work ordinal, runtime instance, workstation generation, workspace, context manifest, invocation/attempt, tool call/execution, artifact, correlation/causation, event stream/version/global cursor, provider request ID, client connection/draft, build/config/schema/protocol versions.
- For a successful tool loop, follow message acceptance, queue/claim, model selection reason, manifest sources/order/size, every invocation/attempt, requested/dispatched tool, privilege/timing/result/artifacts, final commit, event cursor, WebSocket delivery, and client reconciliation.
- Repeat the correlation audit for failure, cancellation, interruption, duplicate response, replay, and restart.
- Compare span fields/logs/metrics with journal/projection rows and artifact metadata; document legitimate cardinality differences such as provider attempts per logical invocation.
- Make the evidence exporter/inspection queries deterministic, read-only, bounded, and redacted.

##### Data/state introduced

Correlation matrix, reconstructed work timelines, reconciliation results, and evidence manifest keyed by immutable IDs and cursors.

##### Contracts/interfaces introduced

Each span/event/row uses the canonical owning identifier; attempt IDs do not replace invocation IDs, PIDs do not replace execution IDs, and connection IDs do not replace replay cursors.

##### Failure behavior

An orphaned, ambiguous, duplicated, or conflicting identifier fails the gate and is fixed at its owning layer; the verifier does not infer a false link.

##### Validation

Automated schema/field assertions plus manual reconstruction of at least one example in every terminal/failure category; totals and order must match SQLite.

##### Exit criteria

- [ ] A reviewer can answer who/what/when/why for every representative work.
- [ ] Attempts, retries, tools, artifacts, and delivery are unambiguous.
- [ ] Derived evidence reconciles exactly to committed history.

##### What is deliberately NOT implemented yet

Cross-service trace federation, a general analytics warehouse, natural-language log search, or telemetry-driven state repair.

#### Substage 30.2: Verify quantitative instrumentation and redaction

##### Objective

Prove that required latency, usage, capacity, truncation, cleanup, replay, persistence, and recovery measurements are present and safe.

##### Why it happens now

Real provider, workstation, network, database, backup, and native-client paths can now generate realistic measurements.

##### Preconditions

Substage 30.1 correlations pass; redaction policy/canaries and baseline workload are defined.

##### Exact implementation work

- Verify measurements for queue/claim/total work duration, model selection, context item/token/byte counts and limits, invocation/attempt/provider latency, first delta, usage tokens, retries, tool queue/dispatch/execute/cleanup duration, privilege, exit/signal, output bytes/truncation/artifacts, cancellation latency, replay gap/count/lag, socket reconnect/backpressure/draft abandonment.
- Verify SQLite busy time, transaction/append/projector latency, WAL size/checkpoints/readers, integrity/startup recovery duration, queue depth, active invariant, disk/artifact/orphan counts, backup duration/head/age/hash, snapshot age, systemd restart count, readiness time, and workstation generation.
- Inject unique fake secret/token/content canaries into config/provider/tool/error paths and scan journald, Caddy logs, application logs, traces, metrics labels, test reports, and client diagnostics.
- Check high-cardinality policy: durable IDs may be structured log/span fields but not unbounded metric labels; command arguments/output/model content stay out of default metrics and are bounded/redacted in logs.
- Record baseline values and measurement units without setting premature SLOs.

##### Data/state introduced

Measurement coverage table, baseline sample, canary/redaction scan report, and any approved redaction exceptions with rationale.

##### Contracts/interfaces introduced

Metric names/units and redaction classification become release-observable contracts; raw secrets are prohibited on every evidence surface.

##### Failure behavior

Any secret leak stops the gate and requires credential rotation if real; missing or nonsensical units/values fail the relevant subsystem's evidence requirement.

##### Validation

Automated expected-metric assertions, bounded-value checks, canary scan with zero forbidden matches, and comparison to canonical timestamps/counters.

##### Exit criteria

- [ ] Every architecture-required measurement is emitted with units and correlation.
- [ ] Metrics avoid unsafe cardinality/content.
- [ ] Redaction canaries do not escape approved secret stores.
- [ ] A quantitative V0 baseline is preserved.

##### What is deliberately NOT implemented yet

SLO/error-budget commitments, user behavior analytics, cost optimization, long-term retention platform, or recording full prompts/output by default.

#### Substage 30.3: Rehearse operational inspection and evidence export

##### Objective

Show that an operator can diagnose runtime health and export release evidence using documented, non-mutating procedures.

##### Why it happens now

The field and measurement contracts are verified and the runbooks exist.

##### Preconditions

Substages 30.1–30.2 pass; least-privilege operator access to the test target is available.

##### Exact implementation work

- Follow runbooks to inspect build/config/schema/protocol versions, readiness/recovery, systemd/Caddy status, runtime/workstation generation, queue/active work, journal head, projector lag/consistency, SQLite/WAL/disk, cgroups/process leaks, artifact integrity/orphans, backup/snapshot age, and recent provider/tool/replay failures.
- Export a bounded release-evidence bundle containing manifests, redacted structured logs/traces, deterministic SQL/query output, selected public events, test identifiers, checksums, and timestamps.
- Prove evidence can be reconstructed after service restart and that absence of tracing does not alter application recovery.
- Peer-review commands for read-only behavior and sensitive-value handling.

##### Data/state introduced

Signed/checksummed evidence-bundle manifest and completed operator inspection checklist.

##### Contracts/interfaces introduced

Operational inspection is read-only by default; any repair/stop/restore action is a separately authorized runbook operation.

##### Failure behavior

Unclear health, a mutating inspection command, incomplete bundle, or an evidence-only dependency in recovery fails the gate.

##### Validation

Fresh operator follows the procedure on the deployed test host, answers the required behavior questions, verifies checksums, and repeats with telemetry temporarily disabled.

##### Exit criteria

- [ ] Inspection procedure is reproducible and non-mutating.
- [ ] Evidence export is complete, bounded, redacted, and checksummed.
- [ ] Recovery remains correct without telemetry.

##### What is deliberately NOT implemented yet

Customer-facing admin console, remote database shell, automatic evidence upload, or production forensic retention policy.

## Stage 31: Exhaustive crash, interruption, and systemd restart verification

### Objective

Exercise every architecture-required crash window and prove that recovery preserves committed truth, never invents an outcome, never repeats an ambiguous side effect, cleans process descendants, and resumes readiness under systemd.

### Why it happens now

Failpoints, deterministic fixtures, real target services, evidence, and recovery semantics all exist. This must pass before acceptance benchmarks create the release record.

### Preconditions

Stages 18, 28, and 30 pass; disposable workspaces/hosts, systemd restart policy, safe side-effect fixtures, and failpoint activation controls are available.

### Exact implementation work

- Execute Substages 31.1–31.4 across local deterministic and deployed target profiles.
- For each failpoint, define pre-state, committed boundary, kill method, expected journal/projection/process/artifact/client state, recovery action, forbidden behavior, and evidence.
- Run failpoints singly from fresh fixtures, then selected repeated/reordered stress cases; never activate them in normal release config.

### Data/state introduced

Crash-run IDs, failpoint activation records outside canonical product history, pre/post journal heads, recovery reports, process/artifact inspection, client replay results, and pass/fail evidence.

### Contracts/interfaces introduced

Crash results are specified by durable transaction boundaries and idempotency keys, not timing guesses. Recovery classifies uncertain external effects rather than retrying them.

### Failure behavior

Duplicate message/work/tool dispatch/final message, false outcome, leaked descendant, corrupted projection, missed replay event, premature readiness, or unrecoverable valid database is release-blocking.

### Validation

Automated crash harness plus real SIGKILL/systemd restart, post-reopen integrity/projector consistency, process/cgroup/artifact inspection, client replay, and evidence review for every row in the crash ledger.

### Exit criteria

- [ ] Every required crash window has deterministic expected and observed results.
- [ ] Ambiguous side effects become `outcome_unknown` and are never auto-repeated.
- [ ] Systemd starts a new runtime generation and gates readiness on recovery.
- [ ] Canonical state, processes, artifacts, and clients remain honest.

### What is deliberately NOT implemented yet

Automatic continuation of interrupted work, distributed failover, exactly-once external side effects, arbitrary power-loss certification, or chaos testing against production.

### Substages

#### Substage 31.1: Verify transactional command, state, and terminal-commit crash windows

##### Objective

Prove atomic visibility and recovery around message acceptance, claiming, context/model/tool intent records, final message commit, cancellation, and graceful shutdown.

##### Why it happens now

These failpoints cover canonical database boundaries and can be validated independently of ambiguous OS/provider effects.

##### Preconditions

Deterministic provider/tools and the precommit/post-transaction failpoint aliases from the planning challenge exist.

##### Exact implementation work

- Test crash immediately before and after the atomic message acceptance transaction; assert either no accepted state or exactly one message/work/input/event/idempotency result.
- Test before/after work claim commit; assert queued work is claimable or old-runtime active work is classified during recovery, never concurrently owned.
- Test context-manifest and model-intent aliases at intra-transaction precommit and after-whole-transaction boundaries; assert no partial logical state and no provider call without committed intent.
- Test before/after model-response normalization commit and tool-request transaction; assert ordered output/tool identity is wholly absent or committed once.
- Test assistant-message terminal transaction aliases; assert assistant message/work terminal/events are all absent or all visible, and replay emits committed message once.
- Test cancellation-request transaction and shutdown checkpoints before claim, between loop steps, and during drain; assert precedence/legal transitions and no new work after drain begins.

##### Data/state introduced

Transactional crash cases with expected row/event counts, stream/global sequences, idempotency response, and runtime ownership.

##### Contracts/interfaces introduced

Named failpoint aliases map to explicit precommit or post-whole-transaction positions; no test weakens an architecture-mandated atomic transaction.

##### Failure behavior

Partial logical state, sequence gaps caused by rolled-back work, illegal transitions, duplicate terminal facts, or ambiguous alias placement fails immediately.

##### Validation

For each case: kill, allow systemd/reopen recovery, run integrity and projection rebuild comparison, submit/replay retry, and compare exact expected rows/events.

##### Exit criteria

- [ ] All database-boundary crash cases are atomic and idempotent.
- [ ] Recovery owns/classifies only committed pre-crash state.
- [ ] Final/cancel/replay facts appear exactly once.

##### What is deliberately NOT implemented yet

Splitting atomic transactions to match a label, automatic repair of corrupt canonical events, or concurrent multi-runtime claims.

#### Substage 31.2: Verify provider-stream and model-response interruption windows

##### Objective

Prove that a crash during provider activity abandons ephemeral drafts, preserves invocation intent/attempt evidence, and safely re-evaluates only where the V0 recovery contract permits.

##### Why it happens now

Provider lifecycle, stateless continuation, drafts, and runtime recovery are all observable.

##### Preconditions

Scripted stream fixtures and a safe live-provider case can pause after request/first delta/before completion.

##### Exact implementation work

- Crash after model intent but before request, after request before first delta, at first provider delta, mid-stream, after complete provider bytes before normalized response commit, and after response commit before next step.
- Persist logical invocation/attempt/request/timing/error evidence according to the committed boundary; never persist a draft as assistant history.
- On recovery, mark old in-flight attempt/intermediate work interrupted/re-evaluable as specified, abandon draft ID, and use a fresh request/attempt from canonical inputs rather than provider conversation state.
- Assert no continuation item from an uncommitted/partial stream leaks into context and `store=false` remains sufficient.
- Repeat one real-provider interruption where safe; use fixtures for exact byte-boundary assertions.

##### Data/state introduced

Interrupted provider attempt records, draft-abandonment notification/evidence, recovery decision, and any fresh attempt under the same/new logical invocation as the architecture specifies.

##### Contracts/interfaces introduced

Provider stream bytes are noncanonical until normalized/committed; drafts are ephemeral; provider request IDs support diagnosis but not recovery identity.

##### Failure behavior

Partial output committed as final, phantom usage, duplicate final message, hidden provider-state dependency, or a draft surviving as truth fails the gate.

##### Validation

Fixture assertions at every stream boundary, systemd recovery, replay/client reconciliation, context manifest inspection, and comparison with a clean deterministic rerun.

##### Exit criteria

- [ ] Every provider crash boundary yields honest invocation/work state.
- [ ] Drafts are abandoned/replaced correctly.
- [ ] Re-evaluation uses canonical state and explicit fresh attempts only.

##### What is deliberately NOT implemented yet

Resuming an HTTP byte stream across process death, provider-side session recovery, semantic deduplication of provider outputs, or automatic infinite retry.

#### Substage 31.3: Verify tool-dispatch, process, and ambiguous-side-effect crash windows

##### Objective

Prove the durable intent-before-effect rule and honest unknown-outcome handling around external OS effects.

##### Why it happens now

This is the highest-risk crash class and depends on real process groups/cgroups, execution inspection, durable tool state, and restart recovery.

##### Preconditions

Safe idempotent/read-only and non-idempotent marker-command fixtures, pauseable tool dispatcher/process helper, and descendant inspection exist.

##### Exact implementation work

- Test crash after tool requested but before dispatch intent, after dispatch intent before handoff, after handoff before spawn observation, immediately after process spawn, while descendants run, and after process exit before durable outcome.
- For never-dispatched committed requests, permit only the explicitly safe recovery path; for any committed dispatch whose outcome cannot be proven, set tool `outcome_unknown` and work `interrupted` without calling the handler again.
- Use an append-only side-effect marker with a unique execution ID to prove at-most-one dispatch after ambiguity; distinguish this external marker from canonical truth.
- Verify process group/cgroup termination, TERM/KILL escalation, reaping, no surviving descendants, and cleanup evidence after backend death/systemd restart.
- Test observed nonzero/timeout/signal results where outcome was durably captured before crash; preserve the structured result without converting it to unknown.
- Reconnect client and verify interrupted/unknown state is delivered durably and no further model/tool step starts.

##### Data/state introduced

Tool/process crash cases, execution/cgroup/process observations, side-effect markers, unknown-outcome terminal records/events, and cleanup evidence.

##### Contracts/interfaces introduced

Durable dispatch intent precedes the effect; execution ID—not PID—is identity; uncertainty is terminal for automatic V0 execution.

##### Failure behavior

A repeated marker, false success/failure, continued descendants, automatic retry, or absence of client-visible interruption is release-blocking.

##### Validation

Exact marker counts, journal/order assertions, state-machine checks, `inspect`/cgroup/process scans, systemd restart, replay, and repeated stress runs.

##### Exit criteria

- [ ] Every dispatch/process crash point has an honest terminal result.
- [ ] Ambiguous non-idempotent work is never automatically repeated.
- [ ] All process descendants are terminated/reaped or explicitly quarantined before readiness.

##### What is deliberately NOT implemented yet

Transactional operating-system effects, generic command idempotency inference, PID-based recovery, user-approved retry UI, or remote execution reconciliation.

#### Substage 31.4: Verify artifact crash cleanup and repeated full-service restart stability

##### Objective

Close the artifact rename/metadata window and prove repeated systemd restarts preserve journal/projection/client consistency.

##### Why it happens now

All isolated crash classes have expectations; this substage tests their composition and cleanup convergence.

##### Preconditions

Artifact orphan reconciler, startup recovery coordinator, systemd target, integrity checker, and replay-capable client fixture exist.

##### Exact implementation work

- Crash before temp fsync, after fsync before rename, after content-addressed rename before metadata transaction, and after metadata commit; verify incomplete temp cleanup, unreferenced-blob quarantine/deletion policy, referenced blob integrity, and no dangling metadata.
- Run sequences of SIGKILL during queued, model, tool, final commit, cancellation, WebSocket delivery, backup, and graceful shutdown; wait for systemd restart/new runtime each time.
- Assert recovery completes once per runtime, old active state is classified, active-work invariant holds, scheduler wakes only after readiness, projectors equal replay, and cursor delivery is gap-free/exactly-once in projection semantics.
- Check WAL/integrity, disk/orphan counts, process/cgroup state, drafts, artifacts, and idempotent resubmission after every restart.
- Preserve a crash-matrix report with seed, build/config, failpoint, expected/actual boundary, and checksums.

##### Data/state introduced

Artifact orphan/quarantine records where applicable, repeated-runtime sequence, recovery measurements, crash matrix, and evidence checksums.

##### Contracts/interfaces introduced

Artifact bytes and metadata become usable only after both sides of their commit protocol reconcile; readiness means recovery and cleanup checks have reached a safe decision.

##### Failure behavior

Dangling referenced artifacts, deletion of valid evidence, restart loop, projector divergence, duplicate/missing client fact, concurrent claim, or leaked process fails release.

##### Validation

Automated artifact failpoint tests plus a prolonged deployed restart campaign, final full integrity/projector/artifact/process/replay audit, and deterministic workload rerun.

##### Exit criteria

- [ ] Every artifact window converges without false metadata or lost referenced content.
- [ ] Repeated restarts preserve one coherent canonical history.
- [ ] Recovery/readiness/scheduler/client order remains correct under composition.

##### What is deliberately NOT implemented yet

Self-healing arbitrary disk corruption, cross-host active failover, external object storage, or indefinite chaos operation.

## Stage 32: Backup, snapshot, restore, and rollback verification gate

### Objective

Prove—not merely document—that canonical state and persistent workstation evidence can be backed up consistently, restored on a replacement Ubuntu VM, and operated or rolled back under the V0 recovery contract.

### Why it happens now

Crash stability and observability are proven, so the release candidate has trustworthy state worth protecting and enough evidence to validate restoration.

### Preconditions

Stages 29–31 pass; off-guest snapshot retention is active; a replacement test VM, prior compatible release, and explicit restore-authority window are approved.

### Exact implementation work

- Execute Substages 32.1–32.3.
- Use benchmark-like state with messages, invocations, tool results, truncated artifacts, workspace files, and completed/interrupted work.
- Preserve source and restored manifests/hashes/heads and enforce that only one environment is authoritative.

### Data/state introduced

Verified online backup, EBS snapshot identifiers, restore-run ID, source/restored comparison, new runtime/workstation generation, rollback drill result, and recovery objective measurements.

### Contracts/interfaces introduced

Restore preserves logical Craxii/conversation/work/artifact identity while intentionally changing runtime instance and workstation generation; a backup never becomes concurrently live.

### Failure behavior

Inconsistent copy, missing committed WAL data, hash/artifact/workspace mismatch, ambiguous authority, schema incompatibility, or failure to reconstruct projections fails the gate and retains the source.

### Validation

Backup under writes, off-guest snapshot, isolated replacement-host restoration, journal replay/projection comparison, artifact/workspace checks, client follow-up, and compatible/incompatible rollback drill.

### Exit criteria

- [ ] A current backup and snapshot are independently verified.
- [ ] Replacement-host restore reconstructs canonical and required workspace state.
- [ ] Identity/generation/authority semantics are correct.
- [ ] Rollback decisions and measured recovery objectives are evidence-backed.

### What is deliberately NOT implemented yet

Hot standby, point-in-time continuous recovery, multi-region replication, transparent failover, zero downtime, or guaranteed disaster-recovery SLA.

### Substages

#### Substage 32.1: Verify consistent backup and off-guest snapshot under load

##### Objective

Demonstrate that backup captures a declared committed journal head while normal short writes and WAL activity occur, and that the off-guest copy is retained independently.

##### Why it happens now

The online backup operation and snapshot automation exist and the state has representative evidence.

##### Preconditions

Writable test target, deterministic workload generator, sufficient disk, backup/snapshot permissions separated as designed.

##### Exact implementation work

- Run command acceptance/model/tool/final/replay traffic while taking an online backup; record source start/end and destination included journal heads.
- Verify destination opens without source WAL files, passes SQLite/application/projector checks, and has declared schema/build/hash/size.
- Confirm every row/event up to the included head and no assertion about later heads; verify referenced artifacts/workspace files through the coordinated data-volume snapshot.
- Trigger/confirm the off-guest snapshot, its encryption/retention/tags/age, and that the guest cannot delete it.
- Interrupt one backup before valid manifest and verify prior valid recovery copies remain and staging cleanup is safe.

##### Data/state introduced

Load-test backup and snapshot manifests, included-head comparison, verification output, interruption evidence, and measured durations/space.

##### Contracts/interfaces introduced

The manifest's included journal head—not wall-clock start/end—is the exact database recovery boundary.

##### Failure behavior

Verification failure invalidates only the new copy; it cannot prune the last known-good backup or snapshot.

##### Validation

Independent reopen/replay, hash and artifact sampling/full check as feasible, source/destination row-by-head comparison, retention/permission inspection.

##### Exit criteria

- [ ] Online backup is self-contained and consistent at its declared head.
- [ ] Off-guest encrypted snapshot contains the coordinated durable volumes.
- [ ] Prior good recovery points survive an interrupted attempt.

##### What is deliberately NOT implemented yet

Byte-identical copy at a later source head, application quiescence for every backup, continuous log shipping, or guest-controlled snapshot deletion.

#### Substage 32.2: Restore to a replacement VM and verify reconstructed operation

##### Objective

Restore the verified recovery point on a separately provisioned target and prove durable-history continuity without concurrent authority.

##### Why it happens now

A trusted backup/snapshot exists and crash recovery semantics are already proven.

##### Preconditions

Replacement VPC/host/DNS isolation, restore authorization, compatible release/config, and source protected from writes or network ambiguity during authority transfer test.

##### Exact implementation work

- Provision a fresh Ubuntu 24.04 x86-64 VM with the same validated filesystem/service prerequisites and attach restored encrypted data volumes/copies.
- Verify manifests/hashes before start; run preflight, schema compatibility, SQLite integrity, journal replay/projector comparison, artifact reconciliation, and process cleanup before readiness.
- Start with a new `runtime_instance_id` and incremented/new `workstation_generation` while preserving `craxii_id`, conversation/message/work/execution/artifact identities and work ordinal history.
- Connect a test client only after explicitly making the replacement authoritative; replay restored history and ask a follow-up that depends on restored durable content using scripted provider, then optionally real OpenAI.
- Verify selected workspace files/tool evidence, backup head boundary, post-restore new writes/cursors, systemd restart, and no source/replacement simultaneous command authority.

##### Data/state introduced

Replacement runtime/generation, restore recovery report, replay/follow-up work after restored head, and authority-transfer record.

##### Contracts/interfaces introduced

Logical identity survives restore; machine incarnation is represented by generation/runtime changes and not hidden path/PID reuse.

##### Failure behavior

Identity rewrite, missing evidence, premature readiness, two writable authorities, or inability to append/replay after restore fails the gate; source remains recoverable.

##### Validation

Compare source manifest to restored database/artifacts/workspace, exact replay through included cursor, run follow-up/tool work, restart, and inspect generations/authority/network.

##### Exit criteria

- [ ] Replacement host reconstructs all state promised by the recovery point.
- [ ] New operation appends safely after the restored head.
- [ ] Identity/generation and single-authority rules are visible and correct.

##### What is deliberately NOT implemented yet

Transparent hostname failover, active-active operation, restoring uncommitted in-memory drafts, or pretending workstation incarnation did not change.

#### Substage 32.3: Verify release rollback and finalize recovery evidence

##### Objective

Prove safe rollback when schema-compatible and explicit backup/restore recovery when it is not, then record measured operational limits.

##### Why it happens now

Both current and restored environments are validated and the release manifest/compatibility contract can be exercised safely.

##### Preconditions

Prior immutable release, current candidate, pre-migration recovery point, and compatibility matrix exist.

##### Exact implementation work

- Deploy current candidate from immutable artifact, run readiness/smoke, then atomically select a prior binary whose declared schema/protocol read/write range includes the current database.
- Restart and verify recovery/replay/read/write; roll forward again and compare journal/projections.
- Attempt an intentionally incompatible rollback in the harness and prove preflight refuses it before database mutation.
- Rehearse the approved incompatible recovery path using the pre-migration backup on an isolated/replacement host, never ad hoc down-migration.
- Record backup/restore/restart/readiness/rollback durations, data-loss boundary, manual steps, failure observations, evidence locations, and owner signoff as V0 baseline rather than an SLA.

##### Data/state introduced

Rollback/roll-forward transcripts, compatibility refusal evidence, recovery-time/data-boundary measurements, and signed restore report.

##### Contracts/interfaces introduced

Binary manifest declares schema/protocol compatibility; deployment tooling must refuse an unsafe combination before service start/migration.

##### Failure behavior

Silent incompatible start, destructive down-migration, loss of newer committed facts during compatible rollback, or unverifiable operator step fails release.

##### Validation

Automated preflight cases plus disposable live compatible rollback/roll-forward and incompatible restore path, followed by full integrity/replay/client smoke.

##### Exit criteria

- [ ] Compatible rollback/roll-forward preserves durable state.
- [ ] Incompatible rollback is blocked and routes through verified recovery.
- [ ] Recovery evidence and measured V0 limitations are reviewable.

##### What is deliberately NOT implemented yet

Universal backward compatibility, automatic down-migration, no-downtime rollout, formal RTO/RPO guarantee, or unattended disaster declaration.

## Stage 33: Full V0.0.01 acceptance-matrix execution

### Objective

Freeze and execute a traceable release matrix covering architecture Acceptances A–J, all automated layers, supported environments, security/boundary checks, operations, and every named benchmark before the canonical release runs.

### Why it happens now

Every subsystem and destructive recovery test has passed its focused gate. A single matrix now prevents an isolated success from hiding a regression or an untested cross-product.

### Preconditions

Stages 24–32 are green; immutable candidate build/config/schema/protocol manifests and clean local, macOS, and replacement-capable EC2 test environments exist.

### Exact implementation work

- Execute Substages 33.1–33.3.
- Map every MUST/MUST NOT and Acceptance A–J clause to a test, inspection, or explicitly reviewed configuration proof.
- Execute the matrix from a clean candidate and preserve machine-readable results/evidence without changing the frozen candidate between passes.

### Data/state introduced

Acceptance traceability matrix, environment/build manifests, test-run IDs, pass/fail evidence, deviations, rerun history, and release-candidate evidence index.

### Contracts/interfaces introduced

Every release claim requires a named reproducible verification and evidence owner; a green aggregate cannot override a failed mandatory cell.

### Failure behavior

Any mandatory failure, skipped required cell, unexplained flake, environment drift, evidence gap, or post-test candidate change invalidates the aggregate and requires a new complete run as scoped by dependency impact.

### Validation

Independent review of traceability and environment provenance; machine-readable result aggregation must agree with raw test/evidence artifacts and exact candidate checksums.

### Exit criteria

- [ ] All architecture requirements and Acceptances A–J have owners and checks.
- [ ] All mandatory matrix cells pass on the immutable candidate.
- [ ] Results are reproducible, redacted, checksummed, and reviewable.
- [ ] Candidate is frozen for named benchmark stages.

### What is deliberately NOT implemented yet

Additional product features, performance optimization, public beta criteria, production-scale load certification, or waiver-based release of failed V0 requirements.

### Substages

#### Substage 33.1: Build the requirement-to-test-and-evidence traceability matrix

##### Objective

Make every frozen architecture requirement visibly testable and prevent benchmark-only coverage gaps.

##### Why it happens now

Focused implementation stages have produced stable test IDs, interfaces, state schemas, and inspection paths.

##### Preconditions

Architecture source hash, this plan, test inventory, candidate manifest, and prior gate reports are available.

##### Exact implementation work

- Enumerate Acceptances A–J clause by clause: real inspection, completed restart, observed tool failure, ambiguous side effect, exact duplicate, queued causal isolation, cancellation, reconnect race, context limit, and restore.
- Add architecture-wide rows for IDs/ownership, legal state transitions, journal/projector consistency, SQL constraints, input closure, artifact durability, process cleanup/privilege, provider normalization/statelessness, replay/drafts, authentication/redaction, schema compatibility, backup/rollback, and migration seams.
- For each row record test ID/type, stage/subsystem owner, local/Ubuntu/macOS/EC2 environment, deterministic/fixture/live mode, setup/seed, exact assertion, database/event/log/artifact evidence, cleanup, expected duration, and mandatory status.
- Mark live external prerequisites separately; never substitute a fixture result for a mandatory real-provider/real-target clause.
- Add negative boundary scans: no SQLx in domain, no OpenAI types outside adapter, no process `Command` outside LocalWorkstation/process adapter, no WebSocket command submission, no absolute-path workspace identity, no PID execution identity, no provider state as history, no raw secret in child environment/evidence.

##### Data/state introduced

Versioned acceptance matrix tied to architecture/candidate hashes and evidence locations.

##### Contracts/interfaces introduced

Traceability status values are `pass`, `fail`, `blocked_external`, or `not_applicable_with_reason`; mandatory release rows may end only in `pass`.

##### Failure behavior

An unowned/unverifiable MUST is treated as failed planning/implementation, not silently omitted or downgraded.

##### Validation

Two-way audit: every architecture acceptance/invariant maps to at least one row and every mandatory test maps back to a requirement and evidence contract.

##### Exit criteria

- [ ] A–J and cross-cutting invariants are completely mapped.
- [ ] Exact assertions and evidence are defined before execution.
- [ ] Boundary/high-migration-cost checks are explicit.

##### What is deliberately NOT implemented yet

Requirements management platform, compliance certification, post-V0 feature tests, or substituting documentation assertions for executable behavior where execution is possible.

#### Substage 33.2: Execute the automated and environment matrix on the frozen candidate

##### Objective

Run every mandatory suite against the exact release candidate in the environments that own its behavior.

##### Why it happens now

Traceability, fixtures, target topology, and immutable release inputs are frozen.

##### Preconditions

Substage 33.1 is reviewed; clean databases/workspaces, test credentials, budget, DNS/TLS, and device provisioning are ready.

##### Exact implementation work

- Run repository policy, formatting, lint, dependency/advisory/license/source-boundary, build metadata, migration and config compatibility checks.
- Run Rust unit/property tests, legal/illegal state transitions, context/token/order tests, canonical model normalization, tool schema equivalence/decoder tests, redaction and secret-type tests.
- Run SQLite migration/reopen/integrity, transaction/idempotency/concurrency, journal sequence/projector replay, input closure, scheduler/recovery, artifact, protocol/replay/WebSocket, failpoint, and backup tests.
- Run LocalWorkstation Ubuntu integration for read, shell, output/artifacts, cwd/env, timeout, process group/cgroup descendants, cancel/inspect/cleanup, sudo/admin compatibility, and structured failures.
- Run scripted provider and OpenAI fixture suites including fragmented streaming, ordered mixed output, function arguments, usage/request IDs, retry classification, stateless continuation, malformed/unknown output, and context limit.
- Run Swift unit/state-reducer/transport/mock-server/UI tests, Xcode build/signing checks, and local real-app integration.
- Run real OpenAI headless, native real-provider, deployed HTTPS/WSS, systemd crash, snapshot/restore, and runbook drills under their approved live profiles.
- Record exact commands, versions, seeds, durations, statuses, logs, state snapshots, and evidence checksums; clean only test-owned fixtures.

##### Data/state introduced

Machine-readable test reports, coverage inventory, environment manifests, redacted logs/traces, state/evidence snapshots, and aggregate matrix result.

##### Contracts/interfaces introduced

Test profile names and required platform ownership become release workflow contracts; live tests are opt-in through explicit credentials/budget but mandatory for release.

##### Failure behavior

Fail fast on deterministic corruption/security/boundary failures; collect independent results where safe. A flaky result is failure until its nondeterminism is explained and the required clean rerun passes.

##### Validation

Compare aggregate to raw exit statuses/test counts/evidence hashes; verify the candidate checksum/build/schema/config remained constant and no test bypassed the public responsibility path.

##### Exit criteria

- [ ] All unit/integration/fixture/native/deployed/live profiles pass.
- [ ] Exact candidate and environment provenance are recorded.
- [ ] No required test is skipped or silently retried to green.

##### What is deliberately NOT implemented yet

Unbounded load/soak, multi-client scale claims, every macOS/Ubuntu version, production credentials, or benchmarks unrelated to V0 correctness.

#### Substage 33.3: Adjudicate results and freeze the acceptance evidence index

##### Objective

Resolve discrepancies, rerun impacted dependencies, and produce the single reviewed index used by final benchmark stages.

##### Why it happens now

Raw matrix results exist and must be judged before ceremonial happy/failure benchmark runs.

##### Preconditions

Substage 33.2 completed with all artifacts retained.

##### Exact implementation work

- Reconcile aggregate rows with journal/projection/database/artifact/log/client evidence and investigate every failure, flake, retry, timeout, skipped test, or environment deviation.
- Classify root cause as implementation, test, environment/config prerequisite, external provider, or evidence defect; fix through the owning earlier stage and rerun every affected downstream gate.
- Prohibit waiver of architecture MUST/MUST NOT and Acceptance A–J requirements; record nonblocking V0 limitations only when outside frozen scope.
- Freeze an evidence index linking each matrix row to candidate/environment IDs, commands, raw result, canonical state bounds, and checksums.
- Confirm benchmark fixtures/conversations start from defined clean state and will not reuse hidden provider/client memory.

##### Data/state introduced

Reviewed disposition log, rerun dependency record, frozen acceptance evidence index, and benchmark starting-state manifests.

##### Contracts/interfaces introduced

Any candidate/config/schema change invalidates affected evidence and requires traceable rerun; a mandatory failed row has no waiver path.

##### Failure behavior

Unresolved discrepancy or evidence that cannot be tied to the frozen candidate blocks progression.

##### Validation

Independent reviewer samples each subsystem and all A–J rows from index to raw/canonical evidence and recomputes checksums/aggregate.

##### Exit criteria

- [ ] All discrepancies are resolved through verified reruns.
- [ ] The frozen evidence index is internally consistent.
- [ ] Named benchmark starting states are clean and declared.

##### What is deliberately NOT implemented yet

Feature exceptions, risk acceptance for failed V0 semantics, marketing claims, or modifying the release candidate during benchmark execution.

## Stage 34: Canonical real-product benchmark

### Objective

Pass the primary machine-inspection and restart-continuity benchmark through the real native macOS app, HTTPS/WSS, EC2 backend, SQLite, OpenAI, agent loop, LocalWorkstation, Ubuntu tools, systemd, and reconstructed durable history.

### Why it happens now

All focused gates and the complete matrix are green. This benchmark now demonstrates the intended product path rather than discovering missing foundations.

### Preconditions

Stage 33 passes; frozen candidate is deployed; real Mac, target EC2, OpenAI key/model, TLS/device token, evidence capture, and a declared clean conversation are ready.

### Exact implementation work

- Execute Substages 34.1–34.2 without test-only service substitution or hidden provider conversation state.
- Use the exact canonical prompts and preserve all canonical/evidence identifiers from acceptance through restart follow-up.
- Require architecture Acceptance A and B criteria in addition to the prompt's semantic correctness.

### Data/state introduced

Canonical conversation/messages/works/inputs/events/manifests/invocations/attempts/tools/artifacts, pre/post-restart runtime records, client cursor/draft evidence, and signed benchmark report.

### Contracts/interfaces introduced

This benchmark is the V0 proof of the whole responsibility chain; a correct answer without the required durable ordering/evidence/recovery is a failure.

### Failure behavior

Wrong fact, missing/duplicate work, hidden tool execution, provider-state dependence, incomplete usage/evidence, false readiness, missed/duplicate replay, or incorrect follow-up fails the benchmark.

### Validation

Observe the UI and independently inspect journal/projections/tool/model/context/artifacts/logs/systemd/replay; compare facts to direct target inspection and execute the post-restart follow-up.

### Exit criteria

- [ ] Canonical first prompt completes correctly through the actual product path.
- [ ] Durable intent/effect/result/final ordering and quantitative evidence are complete.
- [ ] SIGKILL/systemd recovery creates a new runtime and restores history.
- [ ] Canonical follow-up is correct from reconstructed durable context.

### What is deliberately NOT implemented yet

General quality claims, benchmark-specific hardcoded behavior, cached answer injection, provider session reliance, or release signoff before negative benchmarks pass.

### Substages

#### Substage 34.1: Execute and prove the real machine-inspection path

##### Objective

Answer the exact machine-inspection prompt using real OpenAI-selected tool work on the real Ubuntu workstation.

##### Why it happens now

The frozen end-to-end candidate is ready and has no remaining component substitution.

##### Preconditions

Clean conversation/bootstrap cursor, healthy HTTPS/WSS/readiness, idle scheduler, known direct target facts, and evidence capture are confirmed.

##### Exact implementation work

- In the native app submit exactly: “Inspect your machine and tell me what OS, CPU architecture, current directory, and Git version you have.” with one stable client message ID.
- Verify HTTP acceptance returns one durable message/work; SQLite records the trigger input and ordinal; scheduler claims it under the current runtime.
- Verify selected model/target/reason is persisted, context manifest includes only eligible causal sources, and the first OpenAI invocation uses `store=false` and advertised `read_file`/`run_shell` tools.
- Observe ordered model tool request, durable tool request/dispatch, actual Ubuntu command/file operations through Tool Execution Service and LocalWorkstation, structured bounded result/artifacts, and a second model invocation.
- Verify one persisted final assistant message accurately states `/etc/os-release`-derived OS, `uname`/equivalent architecture, configured logical workspace-resolved cwd, and `git --version`, matching direct administrative observation.
- Verify draft is replaced by commit, replay cursor advances, client renders once, and evidence reports invocation count, latency/usage, arguments/privilege/duration/exit/output/truncation/errors.

##### Data/state introduced

The canonical first message/work chain and benchmark fact comparison.

##### Contracts/interfaces introduced

No benchmark shortcut: all commands flow through the public command, scheduler, gateway, tools, workstation, state, and delivery contracts.

##### Failure behavior

Any mismatch or missing durable/evidence link fails; do not edit the answer/state or manually execute on the model's behalf.

##### Validation

UI recording/screenshot as allowed, direct host fact capture, exact row/event/order assertions, trace reconstruction, artifact hashes, and one-message/work count.

##### Exit criteria

- [ ] All four facts are correct.
- [ ] At least one real Ubuntu tool ran through the owned execution path.
- [ ] Every required model/context/tool/client field is durable or observably derived as designed.

##### What is deliberately NOT implemented yet

Prompt special-casing, human correction, extra unsolicited message, or restart/follow-up (next substage).

#### Substage 34.2: Kill, recover, reconnect, and answer from reconstructed history

##### Objective

Prove completed durable history survives backend death and supports the exact follow-up after client reconnection.

##### Why it happens now

The canonical first answer and its full evidence are committed.

##### Preconditions

Substage 34.1 passes; client cursor and pre-kill runtime/journal head are recorded.

##### Exact implementation work

- SIGKILL the backend service process using the controlled operator procedure without graceful in-memory handoff.
- Observe systemd restart, new runtime instance, startup configuration/schema/integrity/recovery/projector/artifact/process checks, scheduler gating, readiness, and unchanged workstation generation unless the machine changed.
- Disconnect/reconnect or relaunch the Mac app; bootstrap/replay from its durable cursor and verify the committed first exchange appears once with no stale draft.
- Disable/delete any provider-side stored conversation assumption; retain `store=false` and build the next request from SQLite/context manifest.
- Submit exactly: “What Git version did you just tell me I have?” with a fresh stable client message ID.
- Verify new ordinal/input closure/context manifest includes the committed prior exchange/tool evidence allowed by policy, excludes unrelated queued messages, and the final answer matches the first committed Git version.
- Preserve pre/post runtime IDs, recovery duration/decision, replay cursors, manifest source list/stats, invocations/usage, and client reconciliation evidence.

##### Data/state introduced

New runtime record, recovery report, follow-up message/work/context/invocation/final answer, and completed Acceptance B evidence.

##### Contracts/interfaces introduced

Continuity derives from canonical SQLite/journal/artifacts and explicit context assembly, never process memory, WebSocket state, or provider conversation storage.

##### Failure behavior

Restart loop, premature readiness, duplicate/missing first message, stale draft, missing history, provider-state dependency, or incorrect Git answer fails release.

##### Validation

Systemd/journald/runtime inspection, database/event/projector comparison before/after, client cursor reconciliation, manifest inspection, and answer comparison.

##### Exit criteria

- [ ] New runtime recovers before readiness.
- [ ] Native client reconstructs committed history exactly once.
- [ ] Follow-up answer is correct from durable reconstructed context.
- [ ] Acceptances A and B pass in full.

##### What is deliberately NOT implemented yet

Automatic resumption of uncommitted model streams, use of provider-held conversation state, workstation reprovisioning, or final release declaration.

## Stage 35: Structured failure and ambiguous-side-effect benchmark

### Objective

Pass the required observed-failure and unknown-outcome benchmarks, proving the product distinguishes known tool results from ambiguous external effects and communicates interruption honestly.

### Why it happens now

The canonical happy/restart path passes; the same frozen candidate must now prove its failure contract without implementation changes.

### Preconditions

Stage 34 passes; safe nonexistent-file/nonzero-command fixtures, unique non-idempotent marker operation, controlled failpoint/SIGKILL, and client/evidence capture are ready.

### Exact implementation work

- Execute Substages 35.1–35.2 in separate declared conversations/workspaces.
- Apply Acceptance C and D exact pass criteria.
- Preserve provider/tool/process/state/client evidence and prove no hidden dispatch retry.

### Data/state introduced

Observed-failure tool/work records and events, ambiguous dispatch/execution with `outcome_unknown`, interrupted work/final client state, marker count, runtime recovery, and benchmark report.

### Contracts/interfaces introduced

An observed unsuccessful result is structured evidence available to the loop; an unobserved/ambiguous effect is not reclassified as failure/success and terminates automatic V0 progress.

### Failure behavior

Backend crash on ordinary tool failure, false success/failure, automatic repeated side effect, generic error that loses classification, or client hiding interruption fails release.

### Validation

Compare durable intent/result/event order, raw safe fixture observations, marker counts, recovery classification, absence of subsequent dispatch/model step, replay, and UI rendering.

### Exit criteria

- [ ] Observed failures remain structured and recoverable/explainable.
- [ ] Ambiguous effect is exactly `outcome_unknown` and work is interrupted.
- [ ] No automatic duplicate dispatch occurs.
- [ ] Replay/native UI represent both outcomes honestly.

### What is deliberately NOT implemented yet

Automatic side-effect retry, generic compensating transactions, user-approved resume/retry workflow, or pretending all commands are idempotent.

### Substages

#### Substage 35.1: Execute the observed tool-failure benchmark

##### Objective

Prove that a tool whose outcome is observed but unsuccessful produces a complete structured result while the service and agent loop remain healthy.

##### Why it happens now

The real loop/tool path passed successfully and failure normalization can be evaluated without crash ambiguity.

##### Preconditions

Fresh work and deterministic prompt/fixture that requests a nonexistent file and/or shell command with known nonzero exit.

##### Exact implementation work

- Submit work through the native product path that causes real `read_file` not-found and/or `run_shell` nonzero execution.
- Verify requested/dispatch/started/outcome order, tool/execution IDs, normalized failure kind, OS error or exit code/signal, bounded stdout/stderr, duration, privilege, truncation/artifact fields, and no transport-level collapse.
- Verify backend stays ready, process cleanup completes, and the model receives the structured result exactly once; it may explain/recover or terminate honestly within loop limits.
- Verify work terminal state and assistant message distinguish tool failure from backend/provider failure and are replayed/rendered accurately.

##### Data/state introduced

Observed unsuccessful tool records/events/results and final model/work/client evidence.

##### Contracts/interfaces introduced

Nonzero/not-found/timeout/permission are observed typed tool results; the exact work outcome follows the explicit loop terminal decision.

##### Failure behavior

Panic/service death, dropped output, false success, unknown-outcome classification despite observed result, or hidden retry fails the benchmark.

##### Validation

Fixture truth comparison, database/event/trace inspection, process cleanup, model input/output, readiness, replay, and native failure rendering.

##### Exit criteria

- [ ] Acceptance C passes for at least one file and/or shell failure representative.
- [ ] Structured evidence is complete and bounded.
- [ ] Backend and subsequent work remain healthy.

##### What is deliberately NOT implemented yet

Automatic correction policy, retries for semantic/command errors, custom UX for every OS errno, or treating stderr alone as failure.

#### Substage 35.2: Execute the ambiguous non-idempotent side-effect benchmark

##### Objective

Prove that death after durable dispatch intent can yield an honest unknown outcome without repeating the operation.

##### Why it happens now

Observed failure semantics are proven and this test isolates ambiguity after an external effect may have started.

##### Preconditions

Fresh work, one-shot append marker with unique execution ID, post-dispatch/process failpoint, and systemd recovery are armed.

##### Exact implementation work

- Submit a native work item whose model requests the controlled non-idempotent marker operation through `run_shell`.
- Pause after committed dispatch intent at the selected boundary where the effect may have happened but before durable outcome; SIGKILL backend and record external marker/process observation independently.
- Allow systemd startup recovery to find the old-runtime dispatched/started tool with no provable durable outcome and transition it to `outcome_unknown`; transition parent work to `interrupted` through legal event/transaction.
- Verify scheduler/agent/Tool Execution Service never redispatches that execution/tool call, even after repeated restart, reconnect, replay, duplicate HTTP retry, or queue wakeup.
- Verify descendant cleanup/quarantine, one-or-zero external marker according to exact kill timing (never more than one), no false success/failure event, and client-visible interruption/unknown detail.

##### Data/state introduced

Ambiguous tool/execution terminal state, interrupted work/event, external marker observation, new runtime/recovery result, and Acceptance D report.

##### Contracts/interfaces introduced

Unknown outcome is a durable terminal safety classification; only a future explicitly authorized new work item could choose a new operation.

##### Failure behavior

Any repeat marker/dispatch, fabricated outcome, automatic continuation, missing interrupted event, or misleading final assistant completion fails release.

##### Validation

Exact dispatch/tool/work row counts and event order, marker content/count, process/cgroup inspection, multiple restart/replay cycles, client UI, and absence of later invocation/dispatch under the interrupted work.

##### Exit criteria

- [ ] Acceptance D passes exactly.
- [ ] Ambiguity remains explicit across restart and replay.
- [ ] Side effect is not automatically repeated.

##### What is deliberately NOT implemented yet

Operator override, compensation, semantic idempotency keys for arbitrary commands, or retrying unknown work.

## Stage 36: Duplicate-submission and idempotency-conflict benchmark

### Objective

Prove that identical client retries/concurrency create one logical command result while reuse of the same key for different payload/auth scope is rejected deterministically.

### Why it happens now

Happy, restart, and failure behavior are frozen; duplicate semantics must hold through the same real network/native/state path before later queue/cancellation benchmarks.

### Preconditions

Stage 35 passes; client/mock concurrency harness and real native retry control can send simultaneous/repeated requests with chosen stable message IDs.

### Exact implementation work

- Execute Substages 36.1–36.2.
- Exercise pre-response disconnect, simultaneous callers, restart/replay, and payload-conflict cases.
- Inspect atomic message/work/input/event/idempotency state and downstream invocation/tool counts.

### Data/state introduced

Idempotency request hash/scope/result evidence, one accepted message/work chain, identical caller responses, conflict error envelope, and benchmark report.

### Contracts/interfaces introduced

The stable client message ID plus authenticated device/command scope identifies one immutable payload; exact duplicates return the original logical acceptance, conflicts never mutate state.

### Failure behavior

Duplicate work/effect/final message, inconsistent duplicate response, accepting a conflicting payload, or dependence on one process's memory fails release.

### Validation

Concurrent HTTP/native attempts, row/event/count assertions, restart retry, downstream model/tool count, public response comparison, and conflict state non-mutation.

### Exit criteria

- [ ] Exact duplicates yield one message/work and same logical result.
- [ ] Conflict is deterministic and creates no new durable work.
- [ ] Semantics survive response loss and backend restart.

### What is deliberately NOT implemented yet

Content-based global deduplication, merging semantically similar prompts, expiring accepted idempotency keys, or exactly-once network delivery claims.

### Substages

#### Substage 36.1: Execute simultaneous and lost-response exact-duplicate submissions

##### Objective

Pass Acceptance E through both raw protocol concurrency and native stable-ID retry behavior.

##### Why it happens now

The full real command path and idempotency transaction are observable and stable.

##### Preconditions

Fresh conversation, one fixed client message ID/payload/device token, synchronization barrier, and a controllable response drop exist.

##### Exact implementation work

- Send two or more byte/logically identical authenticated message commands simultaneously with the same stable ID and normalized payload hash.
- Drop one HTTP response after commit and retry from the native client with the same ID; repeat once after backend restart.
- Verify every caller receives the same logical `message_id`, `work_id`, accepted cursor/status, while SQLite has exactly one user message, one queued work, one trigger input, one acceptance/queue event pair, and one idempotency record/result.
- Allow work to complete and verify one claim, one agent chain, no duplicate tool side effect, one final assistant message, and client reconciliation without a duplicate bubble.
- Verify ordinal allocation and later messages are unaffected.

##### Data/state introduced

One canonical accepted chain plus multiple transport attempt/correlation records and duplicate-hit metrics.

##### Contracts/interfaces introduced

Transport attempts may be many; the durable command result is one and is returned from idempotency state after process restart.

##### Failure behavior

Unique-constraint leakage as generic 500, two work IDs, two ordinals/effects, different success bodies, or client duplication fails the benchmark.

##### Validation

Barrier-based concurrency test, response-drop proxy/fixture, exact database/event/tool counts, restart, replay, and native reducer assertions.

##### Exit criteria

- [ ] Acceptance E passes under true simultaneous submission.
- [ ] Lost response and restart retry return the original acceptance.
- [ ] Downstream responsibility is executed once.

##### What is deliberately NOT implemented yet

Suppressing intentionally distinct IDs, cross-device semantic merge, or relying on HTTP retry middleware to define correctness.

#### Substage 36.2: Execute idempotency-key payload and scope conflicts

##### Objective

Prove that an accepted stable ID cannot be reused to smuggle a different command, conversation, or authenticated scope.

##### Why it happens now

Exact-duplicate success semantics are proven; immutable command identity needs its negative proof.

##### Preconditions

Substage 36.1 canonical acceptance exists and test device/token/conversation scopes are controllable.

##### Exact implementation work

- Resubmit the same client message/idempotency ID with changed text, conversation/workspace, protocol-relevant options, and—where contract applies—a different device/auth scope.
- Verify canonical normalized payload hashing/scoping detects conflict before any message/work/event/ordinal mutation and returns the documented stable protocol error/status.
- Retry the conflict concurrently and after restart; verify it remains deterministic and does not replace the original stored result/hash.
- Confirm logs expose only redacted hash/scope/correlation and no bearer token/raw secret.

##### Data/state introduced

Conflict response/evidence and duplicate/conflict counters; no new canonical product records.

##### Contracts/interfaces introduced

Idempotency identity binds immutable normalized request and authorization scope; conflict is distinct from exact duplicate and validation/auth failure.

##### Failure behavior

Last-write-wins, new work creation, original response mutation, secret leakage, or inconsistent status fails release.

##### Validation

Protocol/integration/native tests, before/after database/journal head comparison, restart repeat, and redaction scan.

##### Exit criteria

- [ ] All meaningful payload/scope changes conflict deterministically.
- [ ] No durable responsibility is added or altered.
- [ ] Original duplicate result remains retrievable.

##### What is deliberately NOT implemented yet

Mutable message editing under the same ID, fuzzy payload equality, idempotency-key garbage collection, or cross-principal key sharing.

## Stage 37: Queued causal-isolation benchmark

### Objective

Prove that a later message can be durably accepted and visible while earlier work is active without leaking into any context or decision owned by that earlier work.

### Why it happens now

Duplicate identity semantics are proven. The scheduler, input closure, context manifests, client queue UI, and delayed tool fixture can now be tested together under real concurrency.

### Preconditions

Stage 36 passes; fresh conversation, deterministic delayed first tool/model fixture, two stable message IDs, and manifest/event/client inspection are ready.

### Exact implementation work

- Execute Substages 37.1–37.2.
- Hold the first work inside a known tool step, accept the second message through HTTP/native UI, and inspect every first-work manifest/continuation before releasing it.
- Then drain the queue and verify chronological ownership, visibility, replay, and context for both works.

### Data/state introduced

Two user messages, consecutive work ordinals, trigger inputs/causal boundaries, delayed first execution, per-invocation context manifests/stats, queue/client/event timeline, and benchmark report.

### Contracts/interfaces introduced

Command acceptance and UI visibility do not imply context eligibility. A work item's closed durable inputs/causal boundary controls all of its invocations.

### Failure behavior

Any second-message content/ID in a first-work manifest/request/tool/final decision, concurrent active claim, invisible accepted queue item, or incorrect ordinal/trigger binding fails release.

### Validation

Pause/barrier test, exact SQLite input/work/state assertions, manifest source hashes/order, captured canonical provider requests, client UI/replay, trace timeline, and post-drain results.

### Exit criteria

- [ ] Acceptance F passes exactly.
- [ ] First work has one immutable input closure across every continuation.
- [ ] Second message is durable/visible/queued and runs only when eligible.
- [ ] No queued-message leakage occurs through model, tool, draft, or final paths.

### What is deliberately NOT implemented yet

Speculative concurrent conversation work, dynamic input widening, implicit interrupt-by-new-message, semantic merging, or hidden queue prioritization.

### Substages

#### Substage 37.1: Accept a later message during delayed active work and prove exclusion

##### Objective

Create the exact concurrency window and demonstrate that every invocation of the active work excludes the queued later message.

##### Why it happens now

Stable idempotency, one-active scheduling, context manifests, and pauseable tools are independently verified.

##### Preconditions

Idle fresh conversation; scripted provider causes first work to enter a delayed foreground tool and later continue to a second invocation.

##### Exact implementation work

- Submit the first message and wait until its tool dispatch/execution is durably active behind a deterministic barrier.
- Submit a distinct second message from the real native app; verify short HTTP acceptance, one new message/work/input/event/ordinal, queued state, and immediate durable UI visibility while the first remains the sole active work.
- Inspect the first work's already committed and subsequently constructed context manifests, selected source IDs/versions/hashes/order, stats, canonical model requests, tool arguments, and drafts; assert no second message, second-work event/result, or its content-derived token/hash appears.
- Release the first tool, allow every first-work continuation/final commit, and repeat the exclusion assertion for each invocation, including any provider retry.
- Verify scheduler never claims the second work until first terminal transaction is committed.

##### Data/state introduced

First active and second queued states, separate trigger-input closures/ordinals, barrier timestamps, per-step exclusion evidence, and client queue projection.

##### Contracts/interfaces introduced

`work_item_inputs` plus the acceptance-time causal boundary are the authoritative eligibility closure; conversation head at assembly time is not.

##### Failure behavior

Re-querying all current conversation messages, mutating first input closure, leaking queued content into retry/continuation, or two active works fails immediately.

##### Validation

SQL/event/manifest/provider-request assertions, content/hash canary for the second message, scheduler ownership timeline, UI state, and repeat with backend wakeup/reconnect noise.

##### Exit criteria

- [ ] Second message is accepted, persisted, queued, and visible during first work.
- [ ] Zero first-work context/request contains the second message.
- [ ] Exactly one work is active throughout.

##### What is deliberately NOT implemented yet

Conversation-wide latest-state context, preemption, priority queue, parallel work execution, or letting live drafts influence another work.

#### Substage 37.2: Drain the queue and verify later-work context and delivery order

##### Objective

Show that after the first work commits, the second is claimed in FIFO order with its own correct causal context and both histories reconcile once.

##### Why it happens now

The critical exclusion window passed and the queue can safely advance.

##### Preconditions

Substage 37.1 first work is terminal and second remains queued.

##### Exact implementation work

- Observe the terminal-first transaction wake the scheduler and claim the second by durable FIFO/work ordinal under the same or recovered runtime.
- Verify second-work context includes its own trigger and only eligible committed earlier history according to policy, including the first final/tool evidence where eligible, but excludes later/uncommitted/draft state.
- Complete the second work and verify independent manifests/invocations/tools/final message and event causation/correlation.
- Disconnect/reconnect a client across the first-terminal/second-claim boundary and verify ordered public events, no duplicate message, correct queued-to-running transition, and no draft crossing work IDs.
- Rebuild projectors and client state from cursor zero to confirm the same order and terminal states.

##### Data/state introduced

Second claim/context/execution/final history and complete two-work replay/client projection.

##### Contracts/interfaces introduced

FIFO activation and context eligibility are related by committed causal facts but remain separately testable; client order derives from durable cursor/IDs.

##### Failure behavior

Second claimed early/out of order, missing its trigger, inclusion of ephemeral/uncommitted facts, duplicate UI state, or projector divergence fails release.

##### Validation

Claim timestamps/transactions, input closures, manifest source comparison, event/global cursor ordering, restart/replay reducer, and exact work/message counts.

##### Exit criteria

- [ ] Acceptance F is complete through queue drain.
- [ ] Second work sees exactly its eligible committed context.
- [ ] Durable and native projections reproduce identical chronology.

##### What is deliberately NOT implemented yet

Queue reordering, cross-conversation scheduling optimization, multi-active concurrency, summary compaction, or draft-to-context promotion.

## Stage 38: Cancellation and process-tree cleanup benchmark

### Objective

Prove cancellation is a durable command with legal precedence, stops a long foreground process tree and future agent steps, survives races/restart/reconnect, and is rendered honestly.

### Why it happens now

Causal queue isolation and unknown-outcome semantics are proven, so cancellation can be distinguished from new-message interruption and backend failure.

### Preconditions

Stage 37 passes; safe long-running process-tree fixture, cancel endpoint/native control, process/cgroup inspection, provider pause points, and systemd are ready.

### Exact implementation work

- Execute Substages 38.1–38.2.
- Run the exact Acceptance G foreground-command case and a bounded cancellation race matrix.
- Verify durable request/event ordering, process cleanup, terminal state, absence of later model/tool activity, replay, and native presentation.

### Data/state introduced

Cancellation command/idempotency record, cancellation-requested/terminal events, tool/process cancel result, signal/cleanup evidence, client states, race dispositions, and benchmark report.

### Contracts/interfaces introduced

Cancellation requests intent durably; the owning runtime observes it at explicit checkpoints and records a legal terminal result. It is not a new conversation message and does not erase history.

### Failure behavior

Leaked child, further model/tool call after terminal cancellation, false completed result, lost/duplicate cancel, illegal transition, or reconnect changing outcome fails release.

### Validation

Native/HTTP cancel, exact journal/state/order/count assertions, process-group/cgroup/reaping inspection, signal escalation timing, repeated cancel/restart/replay, and UI reducer checks.

### Exit criteria

- [ ] Acceptance G passes on the real Ubuntu foreground process tree.
- [ ] No post-terminal model/tool step starts.
- [ ] Cancellation is durable and stable across restart/reconnect.
- [ ] Race cases preserve observed versus unknown outcome semantics.

### What is deliberately NOT implemented yet

Pause/resume, message-based interruption, guaranteed rollback of external effects, background-job management, or user-defined cancellation policies.

### Substages

#### Substage 38.1: Cancel a long foreground command through the real product path

##### Objective

Pass Acceptance G with descendant termination, reaping, durable terminal cancellation, and correct native reconciliation.

##### Why it happens now

The whole cancellation path and real process containment are available on the frozen target.

##### Preconditions

Fresh work whose scripted or safely prompted model invokes a command that creates identifiable child/grandchild processes and waits; no unrelated processes share the cgroup.

##### Exact implementation work

- Submit the work from the Mac app and wait for committed tool dispatch/start with execution/cgroup/process-tree evidence.
- Invoke the native cancel control with stable cancel command ID; verify authenticated short HTTP acceptance and durable cancellation intent/event before runtime action.
- At the tool checkpoint call Workstation cancel, signal the entire execution group/cgroup with TERM then bounded KILL escalation, reap leaders/descendants, capture observed cancelled/signal/timeout/cleanup result, and persist it exactly once.
- Commit work terminal `cancelled` through legal precedence and public event; abort/ignore ephemeral draft and start no later invocation/tool dispatch.
- Inspect `/proc`/cgroup/process table for zero descendants, reconnect/relaunch client, replay the cancellation once, and submit unrelated later work to prove scheduler/backend health.

##### Data/state introduced

Real cancelled execution/tool/work chain, signal/cleanup timings, durable cancel events, client reconciliation, and subsequent-health work.

##### Contracts/interfaces introduced

Foreground execution containment is the cancellation unit; PID is diagnostic only and a clean terminal work state requires a known cleanup decision.

##### Failure behavior

Leader-only kill, orphan/zombie, further loop step, final success message, missing cancel event, or service unavailability fails the benchmark.

##### Validation

Process-tree/cgroup snapshots before/after, exact state/event ordering, invocation/tool counts after cancel, system health, replay, and UI state.

##### Exit criteria

- [ ] Entire process tree is terminated and reaped within the configured escalation bound.
- [ ] Work is durably cancelled once and cannot continue.
- [ ] Reconnect and later work behave correctly.

##### What is deliberately NOT implemented yet

Preserving child jobs, shell-specific job-control UI, undoing completed side effects, or accepting cancellation solely over WebSocket.

#### Substage 38.2: Execute cancellation timing, duplicate, completion, and restart races

##### Objective

Demonstrate deterministic legal outcomes when cancellation races with queueing, claim, model activity, tool dispatch/result, terminal commit, and backend death.

##### Why it happens now

The canonical long-command case passes and can anchor edge-case precedence.

##### Preconditions

Pauseable deterministic provider/tool/transaction fixtures and real process case are available; state-machine race expectations are frozen.

##### Exact implementation work

- Cancel while queued, immediately after claim, before model request, mid-provider stream, after model response before tool dispatch, during a running tool, after observed tool result before next invocation, and concurrently with final terminal commit.
- Send simultaneous/duplicate identical cancellation commands and same-ID conflicting cancellation requests; verify idempotency/conflict behavior.
- Kill backend after cancel intent but before observation and while process cancellation is underway; recover under systemd and apply committed-boundary rules, including `outcome_unknown`/`interrupted` where external effect cannot be proven rather than fabricating `cancelled`.
- Define/assert precedence for already-terminal work: return stable already-terminal/no-op response and never rewrite completion/failure.
- Verify no work leaks active ownership, no late provider delta/draft commits, no post-terminal dispatch, and queue progresses only after a safe terminal/recovery decision.

##### Data/state introduced

Cancellation race table with boundary, winning legal transition, command result, process/tool disposition, and replay/client evidence.

##### Contracts/interfaces introduced

Terminal-state precedence and cancellation idempotency are explicit application contracts; external ambiguity overrides a desired clean-cancel label.

##### Failure behavior

Nondeterministic illegal transitions, rewritten terminal state, false known outcome, duplicate effect/event, late final/draft, or stuck active ownership fails release.

##### Validation

Barrier-based concurrent tests repeated with seeds, exact rows/events, state-machine property checks, process inspection, restart/replay, and native reducer comparison.

##### Exit criteria

- [ ] Every race maps to one documented legal durable outcome.
- [ ] Duplicate cancel and already-terminal cases are stable.
- [ ] Crash ambiguity remains honest and never repeats effects.

##### What is deliberately NOT implemented yet

Distributed cancellation consensus, preemptive thread killing, force-mark-success/failure operator controls, or cancellation compensation.

## Stage 39: Context-limit and reconnect-race benchmarks

### Objective

Pass the remaining explicit acceptance cases: fail oversized full-history context without hidden omission/provider truncation, and deliver a commit occurring between bootstrap and WebSocket synchronization exactly once.

### Why it happens now

All substantive work, queue, and cancellation paths are green. These two boundary tests close context honesty and delivery handoff before final audit.

### Preconditions

Stage 38 passes; configurable model-limit fixture, known eligible history, captured provider dispatch, WebSocket handshake barriers, and native reducer instrumentation exist.

### Exact implementation work

- Execute Substages 39.1–39.2 on the frozen candidate.
- Apply Acceptance I and H exact criteria and retain manifest/cursor/client evidence.
- Rerun normal context and reconnect flows afterward to prove test configuration/failpoints are removed.

### Data/state introduced

Oversized context manifest/stats/failure work, absence-of-provider-dispatch proof, bootstrap high-water/race commit/replay cursors, exactly-once client projection, and benchmark reports.

### Contracts/interfaces introduced

Eligible full history is all-or-explicit-failure in V0; snapshot/replay/live delivery is cursor-based and client idempotence handles overlap without permitting gaps.

### Failure behavior

Silent source omission, provider auto-truncation, hidden compaction, request sent despite known overflow, missed/duplicated committed message in client projection, or ephemeral draft treated as recovery truth fails release.

### Validation

Exact manifest eligibility/source/stats comparison and provider mock absence; deterministic connection barrier with journal cursor/event/client reducer assertions across reconnect and slow/overlap variants.

### Exit criteria

- [ ] Acceptance I passes with explicit `context_limit_exceeded`.
- [ ] Acceptance H passes with the raced commit displayed exactly once.
- [ ] Normal configuration and live delivery remain green.

### What is deliberately NOT implemented yet

Summarization, semantic retrieval, compaction, automatic context dropping, provider truncation fallback, or guaranteed exactly-once network frames.

### Substages

#### Substage 39.1: Execute the explicit context-limit benchmark

##### Objective

Prove an eligible full-history request that cannot fit the selected target fails before provider dispatch with complete omission-free evidence.

##### Why it happens now

Context eligibility and manifests have already passed happy/queue/restart paths; only forced capacity failure remains.

##### Preconditions

Conversation with deterministically tokenized eligible messages/tool items, selected target with deliberately small configured context limit/output reserve, and provider call counter exist.

##### Exact implementation work

- Create/freeze known eligible durable history and submit a new work whose full canonical request exceeds `context_limit - output_reserve` under the target estimator.
- Run normal model selection and Context Assembler; persist the manifest/source IDs/order/hashes and stats required by the architecture, including eligible size, limit, reserve, estimate method/version, and overage.
- Verify no eligible item is silently dropped/reordered/rewritten, no summary/compaction occurs, and tool/system/request overhead is counted according to the estimator contract.
- Fail work explicitly with normalized `context_limit_exceeded` through legal terminal state/event/client error; do not call the provider and do not set provider truncation to an automatic mode.
- Restore normal limit and submit a bounded work to prove configuration isolation and system health.

##### Data/state introduced

Context-overflow manifest/statistics, normalized work error/event, provider-call count evidence, and client rendering.

##### Contracts/interfaces introduced

V0 context capacity failure is explicit and pre-provider; the manifest evidences all eligible sources even though no model request is executed.

##### Failure behavior

Hidden omission, estimator/limit mismatch, provider request, generic failure, retry with less context, or final answer from partial history fails the benchmark.

##### Validation

Independent eligible-source query, manifest/hash/order comparison, estimator recomputation, provider mock/network counter equals zero, journal/UI assertions, and normal-limit follow-up.

##### Exit criteria

- [ ] Acceptance I passes exactly.
- [ ] Full eligible source set and overage are inspectable.
- [ ] No provider truncation/request or hidden compaction occurs.

##### What is deliberately NOT implemented yet

Automatic target switching for capacity, summarization, forgetting, retrieval, source priority dropping, or provider-side truncation.

#### Substage 39.2: Execute the bootstrap-to-WebSocket reconnect race benchmark

##### Objective

Prove a durable assistant commit made after bootstrap snapshot high-water capture but before WebSocket synchronization is neither missed nor duplicated in native state.

##### Why it happens now

Committed event delivery, native cursor persistence, drafts, and barrier-capable server/client fixtures are all stable.

##### Preconditions

Conversation/work poised to commit an assistant message, client with known prior cursor, and controllable barriers around bootstrap snapshot and socket sync exist.

##### Exact implementation work

- Start bootstrap and capture a transactionally consistent snapshot plus high-water cursor; pause before WebSocket replay/live synchronization completes.
- Commit the assistant terminal transaction in the gap and emit/wake notification; record its global cursor.
- Connect/sync WebSocket with client's last applied/high-water semantics; verify server replay covers the gap and any live overlap is allowed at transport level but reduced by event ID/cursor once.
- Verify native state displays one committed assistant message, replaces/abandons matching draft correctly, advances/persists cursor only after apply, and detects/recovers an induced cursor gap.
- Repeat with socket-first notification ordering, disconnect immediately after receive before cursor persistence, slow client/backpressure reconnect, and backend restart between snapshot and sync.

##### Data/state introduced

Bootstrap high-water, raced committed event/cursor, socket/replay connection IDs, client applied cursor/set, overlap/gap/reconnect metrics, and Acceptance H evidence.

##### Contracts/interfaces introduced

Snapshot high-water and durable global cursor form the no-gap contract; WebSocket is a wake/delivery optimization and native projection is idempotent by event identity/cursor.

##### Failure behavior

Missed commit, duplicate message bubble, cursor advanced before application, stale draft overriding commit, silent gap, or need to resubmit command fails release.

##### Validation

Deterministic barrier protocol test, database/public-event cursor assertions, native reducer/storage inspection, repeated orderings, and visual/functional client result.

##### Exit criteria

- [ ] Acceptance H passes for the exact gap race.
- [ ] Overlap, response loss, slow client, restart, and reconnect converge once.
- [ ] Durable replay—not socket memory—repairs every induced gap.

##### What is deliberately NOT implemented yet

Exactly-once WebSocket frames, server-held infinite client buffers, WebSocket commands, durable token-by-token drafts, or multi-device conflict UI.

## Stage 40: Final V0.0.01 release-readiness audit and declaration

### Objective

Perform a final read-only architecture, implementation, security, operations, evidence, and usability audit; freeze the validated release artifacts; and declare V0.0.01 only if every mandatory gate and benchmark passes.

### Why it happens now

All construction and positive/negative benchmarks are complete. The remaining responsibility is to verify nothing drifted, no invariant was bypassed, and the product is genuinely operable from the native client.

### Preconditions

Stages 1–39 and Acceptances A–J pass on the same frozen release candidate/config family; evidence index, deployment/restore artifacts, and known-limitations draft exist.

### Exact implementation work

- Execute Substages 40.1–40.3.
- Reconcile final repository/deployed artifacts with the authoritative architecture and this plan, including every deliberate deferral and migration-cost boundary.
- Freeze signed/checksummed server/client/release/config/schema/protocol/evidence manifests, complete operator/user handoff, and make the go/no-go decision without waiving a mandatory V0 contract.

### Data/state introduced

Final audit checklist, architecture traceability status, boundary/security scans, release manifest/checksums, deployment/restore baseline, acceptance evidence index, known limitations, and release decision record.

### Contracts/interfaces introduced

V0.0.01 release means the exact validated artifacts/configuration and documented target topology; later changes create a new candidate and rerun affected gates.

### Failure behavior

Any open architecture requirement, failed/skipped mandatory test, security/secret issue, nonreproducible artifact, unexplained drift, missing recovery evidence, or unusable native path is a no-go.

### Validation

Independent two-way requirement audit, clean-build/deploy smoke from frozen artifacts, evidence checksum sampling, native usability walkthrough, operational handoff drill, and final A–J/benchmark status review.

### Exit criteria

- [ ] Every required stage, Acceptance A–J, and named benchmark is green.
- [ ] Frozen artifacts are reproducible, deployable, observable, recoverable, and documented.
- [ ] No architecture/security/boundary blocker or unprovided release prerequisite remains.
- [ ] The final native-to-Ubuntu product is actively usable under the V0 contract.

### What is deliberately NOT implemented yet

Anything listed as pre-V1/future architecture: external durable core, remote workstation, production authority service, multi-user/fleet scale, compaction/retrieval, second provider, durable drafts, richer tools, or production operations commitments.

### Substages

#### Substage 40.1: Audit architecture conformance, boundaries, security, and scope

##### Objective

Prove the implementation matches the frozen architecture and preserves the intended long-term seams without accidental generalization or leakage.

##### Why it happens now

All code/config/deployment surfaces and evidence are final enough for a meaningful conformance scan.

##### Preconditions

Authoritative architecture hash, final source/dependency/config/IaC/client inventory, generated schemas, and acceptance index are available.

##### Exact implementation work

- Walk every architecture MUST/MUST NOT, state transition, transaction boundary, failure class, crash window, Acceptance A–J clause, and benchmark requirement against exact implementation/tests/evidence.
- Re-run source/dependency boundary checks: domain has no SQLx/OpenAI/Axum/process/Swift types; OpenAI wire types remain adapter-local; raw `Command`/process APIs remain workstation adapter-owned; application uses intent-specific State Store ports; client commands use HTTP; replay/live use cursor contract.
- Verify logical workspace/workstation/execution identities do not collapse to absolute paths/PIDs; provider/native IDs are diagnostic; tracing/drafts are noncanonical; queued inputs are closed; no side-effect auto retry.
- Audit auth token hashing/constant-time checks/Keychain/source restriction, provider secret type/child environment sanitation, config/systemd credential permissions, TLS/Caddy logs, evidence redaction, absence of production credentials/authority claims.
- Review each deliberate deferral for accidental partial implementation and each implementation-detail choice for unjustified public/durable coupling.
- Run dependency/advisory/license, generated-schema drift, migration immutability/compatibility, API version, build provenance, and unsupported-prototype scans.

##### Data/state introduced

Final conformance/boundary/security/dependency report with requirement links and zero open mandatory findings.

##### Contracts/interfaces introduced

High-migration-cost boundaries become enforced repository tests/ADRs, not reviewer memory; deferred features have explicit non-goals.

##### Failure behavior

Boundary leak, credential exposure, untracked native/durable contract, mutated migration, undocumented generalization, or missing MUST evidence blocks release and routes back to its owning stage.

##### Validation

Automated forbidden-dependency/type/import scans, schema/fixture generation comparisons, secret canaries/scanners, manual architecture walkthrough, and two-way traceability review.

##### Exit criteria

- [ ] Frozen architecture is fully implemented with no contradictory behavior.
- [ ] All high-migration-cost seams are tested and documented.
- [ ] Security/redaction/dependency/migration audits have no release-blocking findings.
- [ ] Deferred scope remains genuinely deferred.

##### What is deliberately NOT implemented yet

Reopening frozen decisions without evidence, abstracting future distributed systems now, production credential delegation, or feature work during audit.

#### Substage 40.2: Freeze reproducible release artifacts and complete user/operator handoff

##### Objective

Make the validated candidate installable, identifiable, supportable, and recoverable without relying on undocumented builder knowledge.

##### Why it happens now

Conformance is clean and the artifact set must be sealed before final declaration.

##### Preconditions

Substage 40.1 passes; server/client signing/checksum/release identities and approved deployment configuration are ready.

##### Exact implementation work

- Produce final Ubuntu-built x86-64 server artifact, native macOS archive/build, migration bundle, Caddy/systemd/config schema/examples, Terraform plan/version lock, backup/restore/deploy/runbooks, and release manifest with hashes.
- Record semantic version/build/Git commit/dependency lock/schema/protocol/min-client/min-server/config compatibility, model target verification date, AMI/instance/filesystem, and exact deployed artifact IDs.
- Rebuild from clean checkout/toolchain lock, compare reproducibility to the declared tolerance, deploy with the runbook, provision/revoke a test device, and run readiness/native canonical smoke.
- Finalize concise native-client connection/use/cancel/reconnect instructions and operator install/deploy/inspect/backup/restore/rollback/incident procedures.
- Document V0 security limitation (server process holds provider credential and autonomous admin capability), single-user/single-workstation topology, unknown-outcome/no-auto-resume behavior, full-history context limit, backup objectives, and other honest limitations.
- Seal acceptance/evidence indexes and store recovery artifacts according to approved retention/access controls.

##### Data/state introduced

Final server/client/release artifacts, signed/checksummed manifest, compatibility/support matrix, handoff docs, known-limitations record, and sealed evidence catalog.

##### Contracts/interfaces introduced

Artifact manifest and compatibility ranges identify what can run together; operator/user docs describe only validated behavior and paths.

##### Failure behavior

Unreproducible/unidentifiable artifact, missing migration/config, undocumented secret/manual step, incompatible client/server, invalid signature/hash, or smoke failure blocks release.

##### Validation

Clean-room build/deploy/restore sampling, checksum/signature verification, fresh Mac connection, device rotation, canonical smoke, and a second operator following only handoff docs.

##### Exit criteria

- [ ] Exact validated artifacts and compatibility metadata are frozen.
- [ ] Clean deploy and native smoke succeed from the handoff package.
- [ ] Operations/recovery/security limitations are explicit and actionable.

##### What is deliberately NOT implemented yet

Public installer/auto-update channel, App Store distribution, hosted control plane, customer support system, formal production SLA, or concealing V0 limitations.

#### Substage 40.3: Make and record the V0.0.01 go/no-go decision

##### Objective

Confirm the end goal in operation and record release only when all evidence, usability, and recovery conditions are satisfied.

##### Why it happens now

The implementation is conformant and its exact deployable artifacts are sealed.

##### Preconditions

Substages 40.1–40.2 pass; final evidence index shows green Stages 1–39 and Acceptances A–J; required owners approve prerequisites/operations.

##### Exact implementation work

- Review the complete stage/acceptance/benchmark status, unresolved defects, external prerequisite log, security/authority limitation, recovery evidence, and candidate hashes in one release meeting/checklist.
- Perform a final native Mac walkthrough over HTTPS/WSS: bootstrap, submit real work, observe queue/draft/tool/final, inspect evidence, cancel a safe long command, reconnect, and verify systemd/readiness/backup age.
- Verify the deployed target can be stopped/restarted/inspected/restored using the sealed runbooks and that no test-only failpoint/provider/config/credential remains enabled.
- If every mandatory item passes, record V0.0.01 release ID/time/artifacts/config/evidence and the next-version boundary. Otherwise record no-go and return to the owning stage without relabeling the candidate.

##### Data/state introduced

Final go/no-go record, release identifier, deployed artifact/config IDs, approval/evidence references, and clean next-version backlog boundary.

##### Contracts/interfaces introduced

A release declaration is immutable evidence about one candidate/topology; future fixes/config changes produce a new declaration and affected reruns.

##### Failure behavior

Any mandatory red item, unknown candidate drift, active test control, inaccessible recovery copy, missing operator/user path, or unjustified approval results in no-go.

##### Validation

All signatories independently sample canonical/failure/duplicate/isolation/cancellation/context/reconnect/restore evidence and observe the final native walkthrough.

##### Exit criteria

- [ ] Go/no-go evidence has no waived V0 requirement.
- [ ] Release artifact/config/topology and limitations are unambiguous.
- [ ] A genuinely usable Craxii V0.0.01 is running through the native macOS client, backed by the real persistent Ubuntu workstation, capable of real model/tool work, observable through logs/tracing/state, restart-safe according to the V0 contract, and passing the complete V0.0.01 benchmark and acceptance suite.

##### What is deliberately NOT implemented yet

Any post-V0.0.01 roadmap item; those begin only after this release is declared and are planned against the preserved seams rather than folded into the V0 candidate.

## Appendix A: Chronological checkpoint map

| Stages | Checkpoint established | No later stage may assume before this checkpoint |
| --- | --- | --- |
| 1–4 | Versioned repository, typed bootstrap, domain IDs/errors, and executable state machines | Durable schema, protocol, provider, tool, or UI behavior |
| 5–8 | SQLite runtime, complete core/journal/evidence schema, deterministic projectors, initial bootstrap, and atomic artifact store | Command acceptance, scheduling, or evidence-backed external effects |
| 9–11 | Authenticated idempotent commands, durable responsibility queue/recovery, HTTP protocol, bootstrap, and replay | Native client or WebSocket-only correctness |
| 12–14 | Workstation port, real Linux file/process/admin behavior, cleanup, Tool Registry, and persisted Tool Execution Service | Model-driven real tool use |
| 15–18 | Canonical model contracts, scripted provider, causal context, explicit bounded loop, and deterministic crash-tested responsibility spine | Real OpenAI or cloud deployment |
| 19–20 | Current Responses adapter, live smoke, ephemeral drafts, and gap-free replay/live handoff | Native transport/UI |
| 21–22 | Native macOS transport, Keychain, durable projection, UI states, cancellation, and client tests | Native end-to-end claims |
| 23–26 | Complete evidence contract plus local deterministic, real OpenAI headless, and native full-path gates | Deployment as a debugging substitute |
| 27–29 | Declarative AWS/Ubuntu release topology, HTTPS/WSS/systemd, backup/restore/rollback assets, and runbooks | Release verification on the target |
| 30–33 | Evidence, exhaustive crash, restore, and full acceptance-matrix verification on a frozen candidate | Canonical release benchmark claims |
| 34–39 | Canonical, failure, duplicate, isolation, cancellation, context-limit, and reconnect benchmarks | Release declaration |
| 40 | Conformance/security/handoff audit and evidence-backed release decision | Post-V0 work |

Execution rule: start a stage only after every preceding stage's exit criteria pass. If a later test exposes an earlier invariant failure, return to the owning stage, produce a new candidate, and rerun all downstream gates affected by that change.

## Appendix B: Required crash-injection ledger

The failpoint controller introduced in Stage 2 names every hook precisely, is compiled/activated only for approved test profiles, records no product-history fiction, and defaults off. Stage 18 proves deterministic behavior; Stage 31 repeats the complete ledger with systemd/target evidence.

| Required window | Introduced around | Expected durable/recovery result | Primary verification |
| --- | --- | --- | --- |
| Message commit | Stage 9 atomic message/work/input/events/idempotency transaction | Precommit: nothing accepted. Postcommit: exactly one retrievable acceptance/work. | Stages 18 and 31.1 |
| Work claim | Stage 10 queue-to-active transaction | Precommit: queued and reclaimable. Postcommit: old-runtime active state classified once during startup. | Stages 18 and 31.1 |
| Context manifest | Stage 17 model-attempt transaction | Intra-transaction death rolls back manifest and intent together; postcommit leaves a complete attempt intent before provider I/O. | Planning Challenge, Stages 18 and 31.1 |
| Model intent | Stage 17 model-attempt transaction | No provider call without committed intent; committed unfinished attempt is explicitly recovered/re-evaluated under a fresh attempt where allowed. | Stages 18 and 31.1–31.2 |
| Provider stream | Stage 19 streaming adapter | Partial bytes/draft are noncanonical; attempt is interrupted/classified, draft abandoned, no partial final message. | Stages 19, 20, and 31.2 |
| Model response | Stage 17 normalized response transaction | Ordered response/tool intents are wholly absent or committed once; no half-decoded continuation. | Stages 18 and 31.1–31.2 |
| Tool requested | Stage 14 request transaction | No handler effect without subsequent durable dispatch intent; a committed undispatched request follows only its explicitly safe recovery rule. | Stages 18 and 31.1/31.3 |
| Tool dispatch | Stage 14 dispatch-intent transaction | Any possibly handed-off effect with no durable observed result becomes `outcome_unknown`; never auto-repeat. | Stages 18, 31.3, and 35.2 |
| Process spawn | Stage 13 LocalWorkstation process lifecycle | Execution ID/cgroup supports cleanup/inspection; ambiguous outcome remains unknown and descendants cannot survive readiness. | Stages 13, 18, and 31.3 |
| Process exit before durable outcome | Stages 13–14 result finalization | Use a durably finalized observation only if provable; otherwise unknown/interrupted, never inferred from absent PID. | Stages 18 and 31.3 |
| Artifact rename before database commit | Stage 8 artifact protocol | Content-addressed orphan is safe to reconcile; no committed metadata points to missing bytes. | Stages 8, 18, and 31.4 |
| Assistant-message commit | Stage 17 terminal transaction | Message/work terminal/events are all absent or all present; postcommit notification/replay delivers once. | Planning Challenge, Stages 18 and 31.1 |
| Cancellation | Stage 10 command plus Stages 13/17 checkpoints | Intent is durable/idempotent; safe terminal or honest unknown wins by legal precedence; no later loop step. | Stages 18, 31.1/31.3, and 38 |
| Graceful shutdown | Stage 10 runtime drain | Stop accepting/claiming, checkpoint/terminate bounded external work, persist honest state, and let next runtime recover before readiness. | Stages 18 and 31.1/31.4 |

Every crash case records: build/config/schema versions; seed; failpoint and physical boundary; pre/post global journal head; runtime/work/tool/execution/artifact IDs; expected and actual rows/events; process/cgroup status; client cursor/projection; systemd restart/recovery timing; and forbidden-repeat checks.

## Appendix C: Acceptance and benchmark traceability

| Frozen acceptance | Construction gates | Final proof |
| --- | --- | --- |
| A — real end-to-end machine inspection | Stages 12–26 and 28 | Stage 34.1 |
| B — completed restart continuity | Stages 7, 10–11, 16–20, 25–26, and 31 | Stage 34.2 |
| C — observed tool failure | Stages 13–18 and 24–26 | Stage 35.1 |
| D — ambiguous side effect | Stages 13–14, 18, and 31.3 | Stage 35.2 |
| E — exact duplicate command | Stages 9–11, 21–22, and 24 | Stage 36.1; conflicts in 36.2 |
| F — queued causal isolation | Stages 9–10, 16–18, 21–24 | Stage 37 |
| G — cancellation | Stages 4, 9–10, 13, 17–18, and 21–24 | Stage 38 |
| H — reconnect race | Stages 11, 20–22, and 24/26 | Stage 39.2 |
| I — context limit | Stages 15–18 and 24–26 | Stage 39.1 |
| J — restore | Stages 5–8, 27–29, and 31 | Stage 32 |

The required final sequence is therefore explicit, not implied by “all tests pass”: Stage 24 full local deterministic integration; Stage 25 real OpenAI headless; Stage 26 native macOS; Stages 27–28 EC2 and HTTPS/WSS; Stage 30 observability; Stage 31 crash/restart; Stage 32 backup/restore; Stage 33 full matrix; Stage 34 canonical benchmark; Stage 35 failure benchmark; Stage 36 duplicate benchmark; Stage 37 queued isolation; Stage 38 cancellation; Stage 39 context-limit and reconnect race; Stage 40 release audit.

## Appendix D: Prerequisite handoff schedule

| Needed by | Owner-supplied or externally selected value | Why it is not an architecture blocker |
| --- | --- | --- |
| Stage 1 | Git repository destination/remote; stable Rust version | Revision hosting/toolchain selection does not alter domain contracts. |
| Stages 13/24 | Disposable x86-64 Ubuntu 24.04 build/test environment with cgroup v2/systemd semantics | It validates a frozen adapter contract; it is not product identity. |
| Stage 19 | OpenAI development project, spend-limited/revocable key, allowed model ID, account rate limits, current API capability/context values | Model target and secret are typed runtime configuration behind a provider port. |
| Stage 21 | Current Xcode, minimum supported macOS, bundle identifier, development team/signing choice | These select a native build target without changing backend protocol semantics. |
| Stage 27 | AWS account/region/AZ, VPC/subnet, x86-64 Ubuntu AMI, instance type, encrypted root/data sizes, KMS policy, billing approval | Infrastructure values are deployment metadata, not durable Craxii/workstation identity. |
| Stage 27 | DNS hostname/control, ACME contact, trusted client source CIDR, optional restricted SSH/break-glass decision | They configure the frozen HTTPS/WSS and trust-boundary topology. |
| Stages 27–28 | Random 256-bit device token/display name and secure out-of-band provisioning channel | V0 auth contract already defines hashing/Keychain/bearer behavior. |
| Stages 29/32 | Backup/snapshot owner, retention approval (planned baseline: daily/14), restore-test VM budget, authority-transfer approver | These activate and govern the defined backup/restore mechanisms. |
| Stages 33–40 | Test windows/budget, final release owners, secure evidence retention location | They enable verification and approval rather than fill an architecture gap. |

All live stages also require an explicit check that production/customer/catastrophic credentials are absent. Missing a row when first needed blocks that stage, not Stage 1 planning or local deterministic implementation.

## Appendix E: Permanent “do not paint ourselves into a corner” checklist

- No SQLx rows/errors/transactions cross the SQLite adapter boundary; application methods express business transactions rather than generic CRUD.
- No OpenAI/Reqwest wire item becomes a domain, journal, context, or public-protocol type; provider conversation storage is never canonical.
- No direct shell/process/filesystem primitive bypasses LocalWorkstation; absolute paths and PIDs remain implementation/diagnostic details.
- No WebSocket frame is a durable command, acknowledgement, or history source; cursor replay repairs loss and clients reduce idempotently.
- No trace/log/draft/client cache participates in recovery or model context unless its content was separately committed through a canonical contract.
- No later queued message enters earlier work through “latest conversation” queries, retries, drafts, tool continuation, or provider state.
- No external side effect occurs before durable intent and none with ambiguous outcome is automatically repeated.
- No tool handler writes journal/state directly, falls back to shell for unknown tools, or silently widens authority/privilege/environment.
- No artifact database reference commits before durable bytes; backend/key/path stays opaque behind `ArtifactStore`.
- No target/model/tool/context/config/schema/protocol change is hidden in an implementation default; version it where compatibility or replay depends on it.
- Do not generalize V0 into a distributed control plane, remote workstation, mature authority service, memory system, parallel agent, multi-provider router, or production trust model. Preserve ports/IDs/manifests so those can be added later without weakening V0 truth.
