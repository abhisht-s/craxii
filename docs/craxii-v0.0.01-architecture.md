# Craxii V0.0.01 architecture

<!-- markdownlint-disable MD013 -->

**Status:** Authoritative implementation source of truth  
**Architecture version:** V0.0.01  
**Document revision:** 5\
**Last updated:** 2026-08-31\
**Audience:** Craxii engineering, Codex, ChatGPT, reviewers, and future contributors  
**Supersedes for V0.0.01:** `CRAXII_V0.0.01_DEEP_ARCHITECTURE_SOURCE_OF_TRUTH.md` and the V0 recommendations in `docs/temp/craxii-v0.0.01-architecture-review.md`

This document is the normative architecture for implementing Craxii V0.0.01. It is not a tutorial, product pitch, or survey of alternatives. It defines what the version must prove, where every important responsibility lives, which state is authoritative, how commands and side effects are ordered, how failures are represented, and which seams must survive later extraction of Craxii's durable core from its first workstation.

The separate credential and identity architecture remains the long-term direction for authority and identity. Where that mature design asks for external control-plane services or project isolation that V0.0.01 explicitly defers, this document governs the V0 implementation.

## How to read this document

The words **MUST**, **MUST NOT**, **SHOULD**, **SHOULD NOT**, and **MAY** are normative.

- **MUST** and **MUST NOT** define acceptance requirements.
- **SHOULD** and **SHOULD NOT** define the default implementation. A deviation requires an architecture note in the pull request.
- **MAY** identifies an allowed implementation choice that does not change product semantics.
- A section labeled **Architecture Challenge** records a deliberate addition or correction to the agreed direction, including its migration impact and whether it blocks V0.

Examples of structures and schemas are normative in meaning, identifiers, constraints, and lifecycle semantics. They are not intended to be pasted into source unchanged. Field names may change only if the resulting contract remains unambiguous and the architecture document is updated first.

## Product definition

Craxii is one persistent, high-agency AI coworker. It should feel like one continuing relationship:

```text
User <-> Craxii
```

The user gives responsibilities, not orchestration instructions. Craxii decides how to investigate, call inference, inspect its computer, invoke tools, persist evidence, recover, and report completion or a genuine blocker.

The model is not Craxii. A frontier model is a replaceable inference engine selected for one invocation. Craxii is the durable system around inference:

- persistent identity;
- durable history and work state;
- context construction;
- model selection and provider adaptation;
- explicit agent-loop control;
- tools and process execution;
- a persistent workstation;
- recovery and interruption semantics;
- authority and privilege decisions;
- artifacts and provenance;
- client delivery and reconnect semantics;
- observability and evaluation evidence;
- later memory, background work, and external authority.

The mature product should preserve one coherent user-facing identity across days, devices, projects, model changes, process restarts, and eventually complete workstation replacement. The V0.0.01 architecture must point toward that product without pretending to implement it.

The workstation is Craxii's computer and replaceable body. Craxii is allowed to administer it broadly. Durable identity roots, catastrophic external authority, recovery roots, and mature canonical state will eventually live outside that body.

> Craxii may own its workstation without the workstation owning Craxii.

## V0.0.01 end goal

V0.0.01 has one end goal:

> Prove Craxii is a real persistent, tool-using agent runtime: it can receive work, act on its Ubuntu machine, persist what happened, restart, and continue from durable state.

This version is successful only when the real native client, real Rust backend, real provider adapter, real Ubuntu machine, real SQLite journal, and real tool execution path work together. A command-line mock alone is not the release benchmark, though deterministic fakes are required during implementation.

V0.0.01 proves four architectural claims:

1. Conversation can be a continuous product surface while `work_item` is the durable execution unit.
2. Model calls and tool side effects can be surrounded by honest durable intent and outcome records.
3. A local Linux workstation can be accessed through a replaceable machine boundary rather than leaked throughout the agent runtime.
4. A native client can disconnect and reconnect without becoming a source of truth or duplicating work.

It does not prove mature memory, production safety, arbitrary in-flight resumption, or workstation-independent identity.

## Scope

V0.0.01 includes:

- one persistent Craxii principal;
- one primary user-facing conversation;
- durable user and assistant messages;
- exactly one work item for each newly accepted conversational user message;
- a durable first-in, first-out queue per conversation;
- at most one active work item per conversation;
- a Rust backend using Tokio;
- Axum HTTP and WebSocket endpoints with selected Tower middleware;
- Serde/JSON boundaries;
- SQLx with SQLite in write-ahead logging mode;
- an append-only domain journal and transactionally consistent current-state tables;
- an explicit agent loop owned by Craxii;
- naive full-history context with exact causal eligibility and explicit limit failure;
- a small model-target and provider-adapter layer;
- one configured OpenAI Responses API target;
- ordered, provider-independent model output items;
- a Tool Registry and Tool Execution Service;
- `read_file` and `run_shell` model-facing tools;
- a small `Workstation` interface and `LocalWorkstation` implementation;
- real Ubuntu filesystem and foreground process execution;
- user-level and administrative execution modes;
- bounded output, timeouts, cancellation, process-tree cleanup, and structured failures;
- idempotent HTTP commands;
- a replayable durable event cursor;
- WebSocket delivery of durable events and ephemeral streaming drafts;
- startup inspection and deterministic classification of incomplete work;
- one native macOS client using Swift and SwiftUI, with AppKit where required;
- one x86-64 Ubuntu 24.04 LTS EC2 workstation;
- encrypted EBS, off-guest snapshots, and systemd supervision;
- structured tracing and enough usage data to drive V0.0.02.

## Non-goals

V0.0.01 deliberately does not include:

- semantic memory or long-term memory projections;
- context compaction or summarization;
- vector retrieval, embeddings, or a vector database;
- full-text search beyond ordinary SQL queries needed by V0;
- scheduled, webhook-triggered, or autonomous background responsibilities;
- steering or merging new input into running work;
- more than one concurrently active work item per conversation;
- multi-agent orchestration, subagents, worker fleets, or distributed scheduling;
- automatic recovery inside an arbitrary provider stream or shell execution;
- automatic retry of ambiguous tool side effects;
- durable interactive terminals or generic daemon supervision;
- browser automation or browser isolation;
- database, cloud, GitHub, or external Model Context Protocol tool integrations;
- multiple real model providers or sophisticated routing;
- provider-owned conversation state as a correctness dependency;
- an external identity, memory, journal, artifact, or Authority Service;
- workstation-independent canonical state;
- production credentials, customer data, or production mutation authority;
- trust realms, project VMs, sandboxes, or multi-tenant isolation;
- policy compilation for natural-language constraints such as “do not deploy”;
- iOS, Android, or Windows clients;
- consumer authentication, teams, billing, or admin consoles;
- PostgreSQL, Redis, S3, Kafka, Kubernetes, Temporal, or an event bus;
- LangChain, LangGraph, or any framework that owns the agent loop.

Deferring these is part of the design. An implementation that introduces them to make V0 “more complete” has expanded scope and requires prior architecture approval.

## Canonical benchmark and evaluation

### Machine-inspection benchmark

The canonical task is:

> Inspect your machine and tell me what OS, CPU architecture, current directory, and Git version you have.

The benchmark passes only if all of the following occur:

1. The native macOS client submits the message through the authenticated HTTP command endpoint.
2. The command carries a stable client message ID used for idempotency.
3. One transaction commits the user message, one trigger relationship, one queued work item, the corresponding journal events, and the idempotency response mapping.
4. The scheduler claims that work item without claiming any other work for the conversation.
5. Model selection chooses the configured OpenAI target before final context rendering.
6. A context manifest identifies every source item rendered for the invocation.
7. A model-invocation intent record commits before the outbound provider request begins.
8. The provider returns one or more ordered output items containing valid tool calls.
9. Tool-call arguments are complete, validated, and associated with the selected work item and workspace by Craxii.
10. A tool-execution intent commits before local execution begins.
11. The command runs on the Ubuntu workstation, not on the Mac and not inside the model provider.
12. Captured stdout, stderr, exit status, timing, privilege, path, and cleanup state are persisted after observation.
13. The tool outcome is rendered into a subsequent model invocation.
14. A final assistant message and `work.completed` transition commit atomically.
15. The client receives durable committed events and may receive ephemeral draft deltas.
16. The displayed answer matches the observed machine facts.

The model is free to choose one or several calls. The benchmark asserts the orchestration and evidence path, not one hard-coded command sequence.

### Restart-continuity benchmark

After the first benchmark completes:

1. Send `SIGKILL` to the backend process or otherwise kill it without application cleanup.
2. Allow systemd to start a new backend process.
3. Verify that the new runtime instance performs startup recovery before becoming ready.
4. Reconnect the Mac client using its last durable cursor.
5. Ask: “What Git version did you just tell me I have?”
6. Verify that a new work item is created and the answer comes from reconstructed durable history, not retained RAM or provider conversation state.

### Failure benchmark

The failure benchmark has two required paths:

- An observed tool failure such as a nonexistent file or nonzero shell exit is persisted as a structured tool result and may be reasoned about by the model without crashing the backend.
- A backend death after a non-idempotent tool intent has been recorded but before a terminal result has committed produces `tool_execution.state = outcome_unknown` and `work_item.state = interrupted` on recovery. The system does not invent success and does not automatically repeat the command.

### Duplicate-submission benchmark

Two submissions with the same device ID, idempotency key, command type, and request material produce:

- one committed user message;
- one work item;
- one pair of acceptance/queue events;
- the original response on every safe retry;
- a deduplication metric but no second domain event.

Using the same idempotency key with different request material returns a conflict and creates no new domain state.

## Architectural invariants

The following invariants take precedence over implementation convenience.

### Identity and ownership

- `craxii_id` identifies Craxii. A conversation ID, model-provider ID, process ID, EC2 instance ID, disk ID, socket, and client device ID do not.
- The backend owns canonical work state and the agent loop.
- The client owns presentation and local user-interface state only.
- The model proposes content and tool calls. It owns neither identity, authority, persistence, scheduling, nor execution.
- The workstation performs machine operations. It does not own conversation, model, journal, or scheduling semantics.

### Durability

- SQLite plus committed local evidence artifacts contains canonical V0 control state.
- Process memory, WebSocket connections, Tokio tasks, provider sessions, and client caches are never canonical.
- The journal is append-only and authoritative for ordered domain transitions.
- Mutable tables are current-state and query projections plus detailed attempt records.
- A domain transition and every current-state mutation it implies MUST commit in one SQLite transaction.
- A client-visible durable event MUST NOT be published before its transaction commits.
- A context window is a rendered view and MUST be reproducible from identified source records.

### Side effects

- Persist external-execution intent before the external side effect.
- Persist the observed terminal outcome after the side effect.
- Never hold a database transaction open while waiting on a model, process, filesystem read, network call, or client.
- Never automatically retry an operation whose side effect may have occurred and whose terminal outcome was not observed.
- A nonzero command exit is an observed command outcome, not a backend infrastructure failure.
- A dropped Tokio future is not process cancellation. Child cleanup must be explicit and verified.

### Work and causality

- Every newly accepted conversational user message creates exactly one work item in the same transaction.
- A retransmission resolves to the already-created work item.
- A protocol control command does not create conversational work.
- At most one work item per conversation may be active.
- Queued messages are visible to the client immediately but invisible to earlier active work-item context.
- Context eligibility is derived from work relationships and ordinals, not “all rows currently in the conversation.”
- Steering, merging, and parallel work are absent in V0.

### Model behavior

- Model selection precedes model-specific context rendering.
- OpenAI wire types do not cross the OpenAI adapter boundary.
- Provider output is an ordered list, not a `FinalText | ToolCalls` union.
- Provider-native continuation data may improve a same-provider loop but is never the only copy of durable history.
- Partial streamed tool arguments are never executed.
- The agent runtime, not the provider's stop field alone, determines terminality.

### Protocol

- Authoritative mutations use idempotent HTTP commands.
- WebSocket is a delivery channel, not the command or continuity substrate.
- Durable delivery has a monotonically increasing replay cursor unrelated to wall-clock time.
- Ephemeral drafts may be lost; committed messages may not.
- A reconnecting client discards stale drafts and converges on committed state.

### Security

- The EC2 VM is one explicit V0 trust and failure domain.
- Craxii has broad local authority, including an explicit administrative path.
- The backend does not run permanently as root.
- V0 Unix paths, users, environment filtering, and cgroups provide operational hygiene, not containment against Craxii with root.
- No production or catastrophic credential may be reachable from the VM.
- Provider secrets are server-side, revocable, spend-limited, omitted from child environments, and never intentionally journaled or traced.
- Recovery copies exist outside the guest VM and are not deletable through any instance authority made available to Craxii.

## Decision supersession

This document resolves conflicts in prior material as follows:

| Prior ambiguity or recommendation | Authoritative V0.0.01 decision |
| --- | --- |
| Conversation turn as execution unit | `work_item` is the execution and recovery unit; conversation is the product surface. |
| One generic Craxii user versus separate restricted executor user | One `craxii` Unix user is used. Separation is hygiene only and is not worth a second process boundary in V0 once Craxii can reach root. |
| Context assembled before model selection | Select a `ModelTarget`, then render context for its limits and capabilities. |
| Binary final-text-or-tool-calls model response | Persist ordered output items; terminality is decided by the runtime. |
| WebSocket submission of user messages | HTTP carries durable commands; WebSocket carries durable replay and ephemeral live events. |
| Cancellation as optional | Cancellation is required because `run_shell` can be long-running. |
| Completed-turn restart test as sufficient | Crash-injection around message, model, tool, final commit, and reconnect boundaries is required. |
| Failed unknown tool outcome | The tool execution becomes `outcome_unknown`; the owning work item becomes `interrupted`. |

## State classification and authority

### Canonical V0 control state

The following bytes are canonical within the V0 workstation failure domain:

- the immutable `craxii_id` and local principal record;
- conversation metadata;
- committed user and assistant messages;
- work items, inputs, ordinals, state transitions, and terminal reasons;
- client-command idempotency records and committed responses;
- journal events and stream sequence heads;
- context manifests and exact source references;
- model invocation intents, normalized outputs, usage, and provider identifiers;
- tool execution intents, outcomes, cleanup evidence, and privilege mode;
- artifact metadata, hashes, and any artifact bytes referenced as canonical evidence;
- runtime-instance and recovery records;
- workstation/workspace logical identities and generations.

“Canonical in V0” means these records decide what the current V0 runtime believes after a process restart. It does not mean they survive total loss or hostile destruction of the workstation and all backups.

### Durable workstation state

The following state persists on EBS and materially affects future engineering work but is not the canonical Craxii control ledger:

- Git repositories and uncommitted changes;
- installed packages, toolchains, and CLIs;
- Docker images, volumes, and local containers;
- local databases and service data;
- user dotfiles and development-environment configuration;
- project-generated files not promoted to canonical evidence.

This state can be irreplaceable in practice. The classification means the journal does not claim to reconstruct it. Workspace snapshots MAY back it up.

### Recoverable backup

A backup is a recoverable copy, not a second active authority:

- EBS snapshots managed outside the guest;
- a SQLite-consistent database backup;
- optional workspace snapshots;
- a pre-migration or pre-release snapshot.

A backup becomes authoritative only through an explicit restore decision. Snapshot recency defines possible data loss.

### Regenerable state

Regenerable state includes:

- build products;
- compiler and package caches;
- context renderings that have retained source manifests;
- search indexes and future memory projections;
- clean repository clones whose remote commit is known;
- derived metrics and UI projections.

Regenerable state may be cached but MUST NOT be the only evidence for a historical claim.

### Ephemeral state

Ephemeral state includes:

- RAM and Tokio tasks;
- locks, notification channels, and cancellation tokens;
- TCP, HTTP, provider, and WebSocket connections;
- streaming text deltas and draft buffers;
- PIDs, process handles, and local execution handles;
- temporary artifact files that have not been committed;
- monotonic timers;
- short-lived working credentials;
- partial SQLite transaction changes;
- uncommitted process output.

Losing ephemeral state must not make committed state internally contradictory.

## Physical topology

### V0 deployment

V0.0.01 uses one AWS EC2 instance in one region and availability zone.

```text
macOS native client
        |
        | HTTPS / WSS
        v
public TLS endpoint, source-restricted
        |
        v
EC2 x86-64 / Ubuntu 24.04 LTS
  |
  +-- TLS reverse proxy
  |
  +-- systemd
  |     |
  |     +-- craxii-server (Rust, User=craxii)
  |
  +-- LocalWorkstation
  |     +-- files
  |     +-- foreground processes
  |     +-- Docker and local services
  |     +-- explicit sudo/admin path
  |
  +-- encrypted EBS
        +-- canonical V0 database and artifacts
        +-- durable workspaces
        +-- caches and machine state

AWS-managed snapshot plane
  +-- off-guest EBS recovery copies
```

The backend and workstation are co-located. The same kernel, root authority, disk, and hosting account form one trust domain. This is deliberate V0 scope, not a mature security claim.

### Compute baseline

- Architecture MUST be x86-64.
- The operating system MUST be Ubuntu 24.04 LTS with security updates applied.
- The instance SHOULD provide at least 4 virtual CPUs and 16 GiB RAM so ordinary compilers, tests, and Docker workloads are representative.
- Instance type is deployment configuration and MUST NOT appear in domain or tool logic.
- The selected Amazon Machine Image ID and provisioning revision MUST be recorded with the workstation generation.
- The VM clock MUST synchronize through the operating system's normal time service.

### EBS layout

V0 SHOULD use two encrypted EBS volumes:

1. A replaceable root volume for Ubuntu and installed system packages.
2. A data volume with `DeleteOnTermination=false` for `/var/lib/craxii`, `/srv/craxii/workspaces`, and durable user-level development state.

Using one encrypted volume is allowed for the first local deployment only if the same directory classifications and snapshot rules are preserved. A second data volume reduces accidental instance-lifecycle coupling; it does not protect state from root on the VM.

### Network layout

- `craxii-server` MUST bind only to loopback in the EC2 deployment.
- A small TLS reverse proxy MUST expose HTTPS and WebSocket Secure on TCP 443.
- The security group MUST restrict TCP 443 to the user's current source range during V0 development.
- SSH, if enabled, MUST be source-restricted. AWS Systems Manager is not required.
- The VM SHOULD have no instance profile. If a hosting role is unavoidable, everything it can do MUST be treated as authority available to root on the workstation.
- Outbound internet access is allowed because package managers, Git, providers, and engineering tools require it.
- The VM MUST NOT have network reachability or credentials for production systems.

## Architecture Challenge: TLS termination was unspecified

**Agreed direction:** Client transport is HTTPS plus WebSocket Secure, but the prior architecture did not identify the component terminating TLS.

**Chosen V0 addition:** Use Caddy as a small systemd-managed reverse proxy in front of a loopback-only Axum listener. Caddy owns certificate acquisition/renewal, TLS policy, request forwarding, and WebSocket upgrade forwarding. It owns no Craxii authentication, commands, state, or business logic.

**Rationale:** Direct TLS in the Rust process couples certificate lifecycle to the application and adds low-value protocol code. An Application Load Balancer adds more AWS infrastructure and cost than a single-user V0 needs. Caddy is a conventional, replaceable edge component.

**Tradeoffs:** It adds one process and one configuration file. A proxy failure makes the client unavailable while the backend may remain healthy. Proxy logs must omit authorization headers and bodies.

**Migration:** A later load balancer, private network gateway, or native Rust TLS listener can replace Caddy without changing the Craxii protocol because the backend receives ordinary authenticated HTTP/WebSocket traffic.

**V0 blocking status:** Resolved by this document. If implementation selects a different terminator, that substitution requires an explicit architecture note and equivalent semantics.

## Architecture Challenge: separate data volume

**Agreed direction:** Canonical V0 state and workspaces live on encrypted EBS attached to the one EC2 workstation.

**Chosen V0 refinement:** Keep operating-system state and Craxii data on separate encrypted EBS volumes when provisioning the shared EC2 environment.

**Rationale:** Reattaching or restoring a data volume is simpler than recovering state intermingled with a failed root filesystem. It makes state classification visible and reduces migration friction without creating a second service or trust boundary.

**Tradeoffs:** Provisioning and mount configuration gain one additional resource. Root on the VM can still corrupt both mounted volumes. Cross-volume snapshots are not automatically application-consistent.

**Migration:** The data paths and stable IDs remain the same when the durable core later moves off-machine. Artifact and state repository interfaces, not the volume, are the lasting seam.

**V0 blocking status:** Not blocking. A one-volume prototype may precede EC2 deployment, but the benchmark deployment SHOULD use the split.

## Trust boundaries and failure domains

### Trust boundary table

| Boundary | Trusted in V0 | Compromise consequence | V0 mitigation |
| --- | --- | --- | --- |
| Native client | User controls the signed development build and Keychain | Attacker can submit work and read conversation state | Device bearer token, TLS, source restriction, revocation |
| TLS proxy | Correctly forwards authenticated traffic and protects transport | Availability loss or traffic disclosure | Minimal config, no body/header logs, loopback backend |
| Rust backend | Owns all V0 control semantics | Complete control-state compromise | Development-only machine, backups, auditable source |
| SQLite/EBS | Correctly persists committed bytes | History/state loss or corruption | WAL/FULL, checks, encrypted disk, snapshots, restore test |
| Local workstation | Broadly trusted to act as Craxii's computer | Can alter machine, read same-domain data, or reach root | No catastrophic credentials; off-guest recovery |
| Model provider | Returns untrusted proposed content/tool calls | Bad plans, prompt injection, unexpected tool requests | Validation, explicit loop, limits, local authority seam |
| Tool child process | Untrusted project/tool code within same V0 domain | Can alter workstation and, with root, the whole VM | Sanitized environment, process cleanup, dev-only authority |
| AWS snapshot plane | Account operations preserve recovery copies | Backup loss if account/control plane is compromised | Instance receives no snapshot-deletion authority |

### Explicit non-boundaries

The following are not strong security boundaries in V0:

- the `craxii` Unix user against a command run as the same user;
- filesystem permissions against an administrative command;
- environment sanitization against a root process that can inspect the host;
- Docker against a user with access to the Docker socket;
- a local socket or helper against root;
- hiding paths from the model;
- the Rust `Workstation` trait against malicious local code;
- an EBS volume against root inside the mounted VM.

These still provide useful operational structure. They must never be described as containing a hostile root-level workstation compromise.

### Failure domains

| Failure | Expected survival | Expected loss or ambiguity |
| --- | --- | --- |
| WebSocket disconnect | All committed state and work continue | Ephemeral draft deltas may be lost |
| macOS app termination | Backend, queued/active work, committed history | Client-only drafts and view state |
| Provider connection failure | SQLite, work intent, context manifest | Provider outcome may be failed or ambiguous; no tool call until complete |
| Tool process failure | Backend and durable intent | Command may exit nonzero, signal, or fail to spawn; outcome is recorded |
| Backend panic/SIGKILL | SQLite/EBS and workspace bytes already persisted | RAM, stream, handles; in-flight side effect may become unknown |
| VM reboot | Encrypted EBS, systemd config, workspaces | Active processes and network state; recovery classifies work |
| EC2 instance loss with intact data volume | Manually reattachable V0 state | No automatic reprovisioning; workstation generation changes |
| EBS data-volume destruction | Off-guest snapshots only | All commits since latest recoverable snapshot |
| AWS account/KMS compromise | No V0 guarantee | VM, snapshots, and encryption authority may be lost |

V0 claims process-restart continuity and manual backup recovery. It does not claim zero recovery point objective, multi-availability-zone availability, or independent survival from AWS account loss.

## Runtime and process topology

### Long-lived processes

The benchmark deployment contains two long-lived product-facing processes:

```text
systemd
  +-- caddy.service
  |     +-- TLS termination and reverse proxy only
  |
  +-- craxii.service
        +-- one Rust process
              +-- HTTP command handlers
              +-- WebSocket delivery manager
              +-- scheduler
              +-- agent loops
              +-- context assembler
              +-- model gateway and OpenAI adapter
              +-- Tool Execution Service
              +-- LocalWorkstation
              +-- SQLite repositories
              +-- artifact store
              +-- tracing
```

Tool commands are child executions, not durable backend services. An explicitly invoked operating-system service such as `systemctl start postgresql` is a workstation side effect managed by systemd, not a child the agent runtime claims to supervise.

### Tokio task ownership

Every spawned Tokio task MUST have an owner and shutdown path.

- Accepted WebSocket upgrades and connection tasks are owned by Craxii transport lifecycle
  accounting. Ownership is reserved before returning Axum's upgrade response; the Axum callback
  transfers that reservation to the active connection supervisor and reports callback failure,
  cancellation, panic, and completion for observation before registry removal.
- Per-work agent tasks are owned by a scheduler `JoinSet` or equivalent task collection.
- Provider stream readers are owned by one model invocation and cancelled with it.
- stdout/stderr drain tasks are owned by one tool execution and joined before terminal persistence.
- notification/broadcast tasks are owned by application state and shut down on service termination.
- No task may be intentionally detached with its result ignored.

Task failure MUST propagate to its owning subsystem. A panic in an agent-loop task must mark the runtime attempt unhealthy and be observed; it must not silently remove active work from memory while leaving its database state running.
At a graceful-shutdown deadline, the scheduler parent MUST abort its remaining runner children and
drain every resulting join record before removing registry ownership or acknowledging scheduler
shutdown. Aborting or dropping the scheduler parent is not a graceful child-ownership mechanism;
unrecoverable process termination is the only catastrophic fallback.

### Runtime instance identity

Every backend process start creates a new `runtime_instance_id` before recovery begins. The runtime record includes:

- UUID;
- Craxii and workstation IDs;
- workstation generation;
- Linux boot ID;
- process ID for diagnostics only;
- binary version and Git revision;
- schema version;
- start time;
- last heartbeat time;
- terminal time and reason when graceful.

PIDs are never used for durable ownership because Linux reuses them. Active work and attempts carry `runtime_instance_id`; startup recovery can therefore identify state owned by a dead process.

`RuntimeInstanceId` is a fresh UUIDv7 for every process start, is never reused, and is durable
ownership identity rather than PID identity. The exact lifecycle is
`new -> running -> stopping -> stopped(graceful_shutdown)`. With the lifetime-exclusive process
lock held, every earlier `running` or `stopping` runtime is stale and becomes
`stopped(startup_failure)` only after its ownership is reconciled. Staleness MUST NOT use heartbeat
age, PID liveness/reuse, boot ID, or lease expiry. Linux reads diagnostic boot ID from
`/proc/sys/kernel/random/boot_id`; non-Linux persists the honest sentinel
`non_linux_not_applicable` and MUST NOT fabricate a Linux host identity.

The owned heartbeat task runs every five seconds, updates only the current `running` runtime,
never moves `last_heartbeat_at` backwards, emits no journal event, and is stopped and joined.
Persistent heartbeat storage failure is fatal/unready and initiates controlled shutdown rather
than silently spinning. Fatal health is terminal presentation evidence, but it does not disable
the shutdown-controller latch: a fatal process keeps health `fatal` while it quiesces claims,
persists stopping/closure when storage permits, and joins owned tasks.

### Readiness

The process has two health concepts:

- **Live:** the HTTP process can answer and has not entered fatal shutdown.
- **Ready:** configuration and schema are valid, database integrity checks passed, startup recovery committed, scheduler started, and the default model/tool configuration is usable.

The TLS proxy may expose liveness without authentication but MUST return no internal detail. Readiness SHOULD be restricted to loopback or authenticated callers.

Application health has exactly `live_unready`, `ready`, `draining`, and `fatal`. Stage 10
production remains `live_unready` after recovery because no production `WorkRunner` exists. A
scripted test composition may become ready after proving recovery, scheduler, and notification
operation. Stage 17 supplies the first real execution-readiness prerequisite; Stage 11 only
exposes this read-only state.

## Technology decisions

| Technology | V0 role | Architectural constraint |
| --- | --- | --- |
| AWS | Hosting and off-guest snapshot control plane | No AWS-specific identity may leak into Craxii domain identity. |
| EC2 | Persistent Linux computer and backend host | It is one replaceable V0 workstation, not Craxii itself. |
| Ubuntu 24.04 LTS | Conventional engineering operating system | OS details are reported as Workstation capabilities. |
| x86-64 | Maximum compatibility with engineering binaries/toolchains | Architecture is configuration/capability, not model logic. |
| Encrypted EBS | Local canonical V0 bytes and durable workstation state | Snapshots are backups; EBS is not mature external canonical state. |
| Rust | Backend, runtime control, protocols, persistence adapters, and process ownership | Rust types enforce boundaries but do not replace durable state or OS cleanup. |
| Cargo | Build, dependency lock, tests, and release artifact production | systemd executes a release binary, never Cargo. |
| Tokio | Async sockets, timers, provider streams, subprocess I/O, cancellation, and owned tasks | Detached tasks and drop-as-cancellation assumptions are forbidden. |
| Axum | Thin HTTP/WebSocket transport adapter | Handlers decode/authenticate/delegate; they do not run the agent loop. |
| Tower | Selected middleware for request IDs, auth, body limits, tracing, and ordinary HTTP controls | Generic timeouts must not break WebSockets or accepted background work. |
| Hyper | HTTP implementation beneath Axum | Do not treat Hyper as a separate subsystem or depend on it directly without a concrete low-level need. |
| Serde and JSON | Versioned command, event, provider-adapter, journal-payload, and config boundaries | Typed structures drive decisions; untyped JSON does not become the domain model. |
| SQLite WAL | Single-host transactional state, journal, projections, idempotency, and attempts | One writer, local filesystem, FULL synchronous durability, no network share. |
| SQLx | Migrations, pool, transactions, and typed SQLite access | SQLx types stay inside the persistence adapter. |
| Reqwest | Outbound provider HTTP and streaming transport | Retry and work semantics remain in Model Gateway. |
| `tracing` | Structured diagnostic spans/events | Traces are not journal history and default to content/secret redaction. |
| systemd | Backend/proxy supervision and service-level cgroup cleanup | It restarts processes; startup recovery restores semantic consistency. |
| Caddy | TLS/WSS edge and loopback reverse proxy | It owns no Craxii authentication or domain state. |
| Swift, SwiftUI, AppKit | Native macOS presentation and transport client | Native client is thin and never owns canonical work/history. |

Versions are pinned in Cargo/Swift dependency manifests and deployment metadata during implementation. This architecture intentionally does not freeze crate patch versions that will age independently of the V0 contracts.

## Rust backend architecture

### Module ownership

The recommended layout is:

```text
backend/src/
  main.rs                     composition root only
  bootstrap/                  config validation, migrations, recovery, startup
  domain/
    ids.rs                    typed durable identifiers
    craxii.rs                 principal and display identity
    conversation.rs           conversation/message rules
    work.rs                   work state machine and input relationships
    journal.rs                event envelope and event payload types
    model.rs                  canonical model types
    tool.rs                   canonical tool contracts
    workstation.rs            machine request/result types
    artifact.rs               artifact identity and metadata
    protocol.rs               Craxii-owned public protocol types
    error.rs                  stable normalized error taxonomy
  application/
    command_service.rs        idempotent client commands
    scheduler.rs              durable FIFO claim and task ownership
    agent_loop.rs             explicit model/tool loop
    context_assembler.rs      eligibility, rendering, manifest creation
    model_gateway.rs          selection, attempts, retry orchestration
    tool_execution.rs         intent/policy/dispatch/outcome ordering
    cancellation.rs           cancellation coordination
    recovery.rs               incomplete-state classification
    event_delivery.rs         durable replay + ephemeral draft fan-out
  ports/
    state_store.rs            transactional state/journal operations
    artifact_store.rs         content-addressed evidence storage
    model_provider.rs         provider invocation boundary
    workstation.rs            low-level machine boundary
    clock.rs                  wall and monotonic time seam for tests
  adapters/
    sqlite/                   SQLx migrations, repositories, projections
    local_artifact_store/     EBS-backed artifact bytes
    openai/                   Reqwest, wire types, stream decoder
    local_workstation/        filesystem/process/cgroup implementation
    http/                     Axum commands, auth, bootstrap
    websocket/                event protocol and reconnect delivery
    telemetry/                tracing initialization and redaction
```

Names may change, but ownership may not.

### Dependency direction

```text
transport adapters ----+
provider adapter -------+
SQLite adapter ---------+--> application --> domain
workstation adapter ----+
```

- Domain modules MUST NOT import Axum, SQLx, Reqwest, OpenAI wire types, Swift protocol details, or Linux process types.
- Application modules may depend on domain types and narrow ports.
- Adapters implement ports and translate external representations.
- `main.rs` constructs concrete adapters and injects them into application services.
- A trait is justified only at a real replacement or test boundary. Internal helper modules SHOULD use concrete types.

### Component responsibility table

| Component | Owns | Must not own |
| --- | --- | --- |
| HTTP adapter | authentication extraction, request limits, JSON decoding, status mapping | work creation transactions, scheduling, agent iteration |
| Command Service | idempotency, atomic message/work/cancel commands | WebSocket connections, provider calls, tool execution |
| Scheduler | durable FIFO claim, one-active invariant, task ownership | context content, provider translation, tool validation |
| Agent Loop | work progression, loop limits, terminal decision | SQL syntax, HTTP details, Linux spawn details |
| Context Assembler | causal eligibility, canonical context package, manifest | memory persistence, provider HTTP, scheduling |
| Model Gateway | target selection, invocation attempts, provider retry policy | client protocol, tool execution, conversation identity |
| Provider Adapter | auth, wire translation, streaming decode, provider errors | work scheduling, canonical history, tool dispatch |
| Tool Execution Service | validation, policy seam, intent/outcome transactions | provider schema translation, machine implementation |
| Tool Registry | definitions and handler lookup | persistence, scheduling, policy, retries |
| Tool Handler | typed tool semantics | journal writes, provider calls, client delivery |
| Workstation | low-level machine operations | model-facing schemas, work state, authority policy |
| State Store | transactions, queries, event append, projection integrity | business transition decisions outside passed commands |
| Event Delivery | cursor replay, live wakeups, draft delivery | canonical event creation |

### Dependency governance

The agreed stack does not authorize arbitrary crates. Every nontrivial dependency added during implementation MUST have a short record explaining:

- the primitive it supplies;
- why standard library or an already-approved crate is insufficient;
- whether it handles secrets, parsing, persistence, or unsafe code;
- maintenance and security posture;
- the layer in which it is allowed;
- removal/migration cost.

No dependency may own the agent loop or encode provider, workflow, memory, or tool policy on Craxii's behalf.

## Domain model

### Aggregate map

```text
CraxiiPrincipal
  +-- Conversation
  |     +-- Message
  |     +-- ordered WorkItems
  |            +-- WorkItemInputs -> JournalEvents
  |            +-- ContextManifests
  |            +-- ModelInvocations
  |            +-- ToolExecutions
  |            +-- AssistantMessage
  |
  +-- Workstation
        +-- Workspace
        +-- generation

JournalEvent
  +-- globally ordered by journal_offset
  +-- ordered in an aggregate stream by stream_seq
  +-- links causation, correlation, conversation, and work

Artifact
  +-- content-addressed bytes
  +-- metadata and provenance in SQLite
```

### Identifier policy

All public and durable entity IDs MUST be opaque UUIDv7 values represented as lowercase canonical strings at JSON and SQLite boundaries. Time ordering from UUIDv7 is convenient for inspection but MUST NOT be used as canonical ordering.

Required typed identifiers include:

- `craxii_id`;
- `conversation_id`;
- `message_id`;
- `work_id`;
- `workstation_id`;
- `workspace_id`;
- `runtime_instance_id`;
- `journal_event_id`;
- `model_invocation_id`;
- `logical_invocation_id` for grouped retries;
- `context_manifest_id`;
- `tool_execution_id`;
- `artifact_id`;
- `device_id`;
- `client_command_id` or idempotency key;
- `correlation_id`;
- `draft_id` for ephemeral output.

The Rust domain MUST use newtypes so a work ID cannot be accidentally passed where a conversation ID is expected.

`correlation_id` MUST be represented by the distinct canonical UUIDv7 `CorrelationId` newtype. It is not interchangeable with any entity ID type, and its embedded timestamp or lexical order MUST NOT be used as FIFO, causality, replay, lifecycle, attempt, journal, or work ordering authority.

Global and stream ordering use SQLite integers:

- `journal_offset` is the global durable replay cursor.
- `stream_seq` is ordering inside one aggregate stream.
- `conversation_work_ordinal` is FIFO order of work created from one conversation.
- `agent_step_no`, `tool_ordinal`, and retry attempt numbers order records within one work item.

No order relies on timestamps.

### Time policy

- Persist wall-clock timestamps as UTC RFC 3339 strings with microsecond precision.
- Record both `recorded_at` and an external `occurred_at` only when an external event supplies meaningful occurrence time.
- Use a monotonic clock for durations, deadlines, and latency.
- Never compute timeout expiry from a persisted wall-clock difference alone.
- Time values are evidence and presentation; ordering remains sequence-based.

### Craxii principal

The V0 principal is one immutable domain snapshot with an immutable `craxii_id`. It is created once during initial database bootstrap, before the primary conversation.

The snapshot contains:

- immutable `craxii_id`;
- `display_name` and `owner_label`, each preserving exact internal UTF-8 spacing while requiring 1..=128 UTF-8 bytes, no NUL or control characters, and no leading or trailing whitespace;
- lifecycle state `active`;
- `primary_conversation_id`;
- `default_workspace_id`;
- `created_at`;
- `architecture_revision` and positive `schema_revision` at creation.

The Stage 3.2 snapshot is immutable. V0 display-name changes, if later supported, produce a new guarded projection rather than mutating this value in place.

The V0 principal has no permanent private key. Its continuity is currently the continuity of the SQLite row and its recoverable backups. This is explicitly weaker than mature Craxii identity.

The following MUST NOT define identity:

- OpenAI account, API key, response ID, or model;
- EC2 instance ID, hostname, AMI, or EBS volume ID;
- systemd unit or Unix account;
- a conversation ID;
- a native-client installation;
- an in-memory singleton.

When the workstation is replaced later, the same `craxii_id` is assigned to a new workstation generation by the external durable core.

### Conversation

Exactly one visible primary conversation exists per Craxii principal in V0. `primary` is the only V0 `ConversationKind`, and `active` is the only V0 conversation lifecycle value. Stage 3.2 creates no hidden or system conversations. Application topology validation enforces the singleton primary relationship; later schema/bootstrap stages enforce database uniqueness. Schema extensibility does not guarantee multiple-conversation behavior in V0.

A conversation owns:

- a stable ID and `craxii_id`;
- kind `primary`;
- lifecycle state `active`;
- `created_at`;
- `next_work_ordinal`, allocated transactionally;
- a positive `projection_version` for guarded updates.

A conversation does not own model-provider session state. It is not the execution primitive. It is a durable product surface that orders accepted messages and associated work.

Conversation title/display metadata, multiple visible conversations, a thread picker, and conversation-management behavior are deferred.

### Message

A message is committed user-visible conversation content. V0 content is text-only and uses `ContentVersion = 1`. `ContentBlock` has exactly one V0 variant, `Text(String)`. A message contains at least one block; each text block contains at least one UTF-8 byte; whitespace-only text is valid and is not trimmed; block order is significant; and the combined UTF-8 text payload across all blocks is at most 65,536 bytes. Text normalization is identity: no Unicode, newline, whitespace, or case normalization occurs. Images, files, structured blocks, and multimodal blocks are deferred.

Conceptual content:

```text
Message
  message_id
  craxii_id
  conversation_id
  role = user | assistant | system
  content_blocks[]
  produced_by_work_id?
  client_device_id?
  client_message_id?
  content_sha256
  committed_at
```

Rules:

- Exact roles are `user`, `assistant`, and `system`; there is no developer or tool message role.
- A user message requires paired `DeviceId` and `ClientMessageId` values and has no `produced_by_work_id`.
- An assistant message has neither client provenance value and requires `produced_by_work_id = Some(WorkId)`.
- A system message has neither client provenance value and no `produced_by_work_id`.
- A user message is committed only inside the message/work creation transaction.
- An assistant message is committed only when the agent runtime decides the work has a terminal user-facing response.
- Streaming deltas are not messages.
- A failed or abandoned provider draft never becomes a message.
- Message rows are immutable after commit. Corrections append a later message/event.
- User content MUST NOT be duplicated in context as both transcript history and a separate current prompt.
- The unique pair `(device_id, client_message_id)` prevents the same client message from creating two domain messages.
- A message has no reply linkage, message `CorrelationId`, or separate `created_at`.

#### Canonical content bytes and hash

The storage-neutral canonical byte grammar is frozen exactly as follows. All integer fields use unsigned big-endian binary encoding:

```text
ASCII "craxii.content"
u8 content_version = 0x01
u32 block_count
for each block in order:
  u8 block_type = 0x01        # text
  u64 utf8_byte_length
  exact UTF-8 bytes
```

No terminator is appended. The content hash is `SHA256(canonical_bytes)`. The explicit version byte, block count, block type, and byte-length prefixes are collision-separation fields. Serde JSON MUST NOT be used as hash input.

`content_sha256` digests content blocks only. It excludes `MessageId`, `CraxiiId`, `ConversationId`, role, `committed_at`, work/client/device/correlation identifiers, JSON/object ordering, and storage metadata.

### Work item

`work_item` is the internal unit of responsibility, scheduling, execution, cancellation, and recovery.

The immutable Stage 3.2 structural reference contains creation/topology fields only:

```text
WorkItem
  work_id
  craxii_id
  conversation_id
  conversation_work_ordinal
  kind = conversational
  priority = 0
  workspace_id
  correlation_id
  created_at
  queued_at
```

Stage 4 owns lifecycle/state fields and transitions. `WorkItem` contains no triggering `MessageId` foreign-key field.

The work item is deliberately distinct from:

- the user message that triggered it;
- one model invocation;
- one tool execution;
- one provider response;
- the full conversation;
- a future background scheduler job;
- a future agent or worker.

One work item may contain several model invocations and several tool executions. V0 processes it in one backend runtime task at a time.

### Work-item inputs

Inputs are modeled as a relation, not a single non-null `user_message_id` column on `work_items`.

```text
WorkItemInput
  work_id
  input_event_id
  relationship
  ordinal_within_work
  attached_at
  actor
```

The exact relationship values are `trigger`, `steering`, `supplemental`, `scheduled_trigger`, `external_trigger`, and `recovery_instruction`. The closed Stage 3.2 `WorkInputActor` vocabulary is `user`, `craxii`, `system`, and `recovery`; it records relationship provenance and is not automatically identical to a later journal actor DTO.

The V0 conversational-work application constructor requires exactly one input total, relationship `trigger`, `ordinal_within_work = 1`, and a `WorkId` matching its `WorkItem`. The event's semantic type `message.accepted` is verified only in later persistence/application transaction stages because Stage 3.2 cannot infer an event type from `JournalEventId` alone.

Reserved relationships remain structurally representable but are rejected by the V0 application constructor and client path.

### Workstation, workspace, and path identity

`WorkstationGeneration` is a distinct positive integer in `1..=i64::MAX` with numeric Serde, semantic equality/order/hash, and checked increment. A process restart does not change it. Replacement, restore, or reprovision does change it. It is scoped with `WorkstationId` and MUST NOT reuse another sequence wrapper.

The immutable `WorkstationIdentity` contains `WorkstationId`, `CraxiiId`, kind `local`, `WorkstationGeneration`, a bounded opaque `HostingProvider`, optional provider instance/image/provisioning revision evidence, CPU architecture, OS release, and `created_at`. Hosting evidence does not define identity. PID and hostname are excluded.

`WorkstationCapabilitiesVersion` is exactly `1`. The immutable snapshot contains workstation ID/generation/kind, CPU architecture, OS release, default shell, booleans for filesystem read, foreground execute, cancellation, inspection, user/admin privilege, process-group cleanup, and cgroup cleanup; nonnegative signed-64-bit-safe maximum execution timeout/stdout/stderr bounds; and an ordered `Vec<WorkspaceCapabilityRef>` of workspace ID and logical root. Duplicate workspace IDs are invalid. Capabilities describe machine ability and never grant authority. Stage 3.2 adds no generic network capability and freezes no public canonical JSON for the aggregate.

The immutable `WorkspaceIdentity` contains `WorkspaceId`, `CraxiiId`, `WorkstationId`, stable logical name/root, lifecycle `active`, and `created_at`. Workspace identity survives generation changes; a resolved machine root is not workspace identity.

#### Logical path grammar

Stage 3.2 paths are POSIX-oriented because the V0 workstation is Ubuntu/Linux. `LogicalPathReference` preserves an explicit kind (`workspace_relative` or `absolute`) and a canonical UTF-8 string of at most 4,096 bytes. NUL and backslash are rejected. Canonical identity MUST NOT use `PathBuf`, filesystem access, symlink resolution, or filesystem canonicalization.

Workspace-relative input MUST NOT start with `/`. Repeated `/` collapses, `.` segments are removed, and `..` pops one prior normal segment; escaping above the workspace root is invalid. At least one normal segment must remain. The result joins segments with `/` and has no trailing slash.

Absolute input MUST start with `/`. Repeated `/` collapses, `.` segments are removed, and `..` pops one prior normal segment; `..` at filesystem root remains clamped at root. Canonical root is `/`; every other canonical absolute path starts with `/` and has no trailing slash.

Existing-target and symlink resolution belong to the later Workstation adapter. `ResolvedPathEvidence` is only adapter-observed physical evidence: `WorkstationId`, `WorkstationGeneration`, `WorkspaceId`, requested `LogicalPathReference`, and a redacted physical UTF-8 `resolved_absolute_path`. The physical value must be syntactically absolute POSIX text, contain no NUL, and be at most 4,096 bytes. It has no timestamp, inode/device/symlink-chain data, authority semantics, or Stage 3.2 Serde contract; safe `Debug` output MUST redact the physical path. It never replaces the workspace ID/requested path.

### Bounded domain references and immutable evidence

Stage 3.2 uses narrow concrete bounded-string types, not a public generic stringly typed identifier.

- `ProviderId`, `ModelTargetId`, and `ToolName` are 1..=64 ASCII bytes, lowercase, start alphanumeric, and thereafter contain only lowercase alphanumeric, `.`, `_`, or `-`.
- `ProviderModelId` is 1..=128 UTF-8 bytes, has no leading/trailing whitespace or ASCII control character, and is preserved exactly without normalization.
- `ToolVersion` is 1..=64 visible ASCII bytes with no whitespace/control characters.
- `SchemaVersion` is a distinct positive signed-64-bit-safe numeric wrapper.
- `AuthorityReasonCode` is 1..=64 lowercase ASCII bytes, starts alphabetic, and thereafter contains only lowercase alphanumeric or `_`.

These grammars are canonical domain references and MUST NOT be derived from bootstrap configuration enums.

#### Provider/model reference

`ProviderModelReference` is neutral evidence, not Stage 15 target configuration. It contains `ModelTargetId`, `ProviderId`, `ProviderModelId`, a positive target-configuration version, and an immutable `ModelCapabilitySnapshot`. The capability snapshot contains booleans for text input/output, custom tool calling, streaming, ordered output items, structured output, and reasoning continuation, plus positive signed-64-bit-safe context-window and maximum-output token counts. It contains no pricing, credential/account reference, provider wire ID, or provider SDK type.

#### Artifact reference

`ArtifactReference` is immutable metadata only: `ArtifactId`, `CraxiiId`, optional producing `WorkId`, one `ArtifactProducer`, storage backend `local`, opaque storage key, `Sha256Digest`, canonical byte length, optional observed length, MIME type, optional encoding/logical name, retention, truncation flag, optional compression, and `created_at`.

`ArtifactProducer` is exactly `none`, `model(ModelInvocationId)`, or `tool(ToolExecutionId)`, preventing simultaneous model/tool identity. Broader work provenance may coexist with a specific producer; there is no XOR with `producing_work_id`. Retention is `canonical_evidence`, `diagnostic`, or `regenerable`. V0 freezes `ArtifactStorageKey` to `sha256/<2 lowercase hex>/<64 lowercase hex>`; it is a logical content key, never an absolute path or client URI. Stage 3.2 performs no filesystem I/O.

#### Authority and attempt references

`AuthorityDecision` is `allow` or `deny`; `PrivilegeMode` is `user` or `administrative`. `AuthorityDecisionSnapshot` contains the decision, effective privilege, policy version exactly `v0-development-workstation`, and `AuthorityReasonCode`. It has no token/secret/credential, argument payload, evaluator, or Stage 3.2 public Serde contract. Stage 14 owns evaluation and richer structured evidence.

`ModelAttemptReference` contains `LogicalInvocationId`, `ModelInvocationId`, `WorkId`, `RuntimeInstanceId`, `ContextManifestId`, `AgentStepNo`, `AttemptNo`, `ProviderModelReference`, and optional `retry_of: ModelInvocationId`. It has no state, outcome, provider request/response ID, usage, or lifecycle timestamps.

`ToolAttemptReference` contains `ToolExecutionId`, `ExecutionId`, `WorkId`, `RuntimeInstanceId`, source `ModelInvocationId`, `AgentStepNo`, `ToolOrdinal`, `ToolName`, `ToolVersion`, `SchemaVersion`, `WorkstationId`, `WorkstationGeneration`, `WorkspaceId`, optional requested `LogicalPathReference`, and `AuthorityDecisionSnapshot`. Provider-native tool-call ID, PID, process state, and outcome are excluded.

`RuntimeStartEvidence` contains `RuntimeInstanceId`, `CraxiiId`, `WorkstationId`, `WorkstationGeneration`, optional bounded Linux boot ID and diagnostic PID, bounded package version and git revision, `SchemaVersion`, and `started_at`. `RuntimeInstanceId` is canonical identity; PID/boot ID are diagnostic only. Hostname, heartbeat/state/stopped fields, persistence behavior, PID lookup, and boot-ID reads are excluded. Domain code MUST NOT import bootstrap build metadata wholesale.

The persisted Stage 10 runtime start requires both diagnostic fields: Linux supplies the actual
kernel boot ID and non-Linux supplies `non_linux_not_applicable`; PID is the positive current
process value. Test composition may inject either observation. The narrow runtime-observation port
contains no hostname, path, liveness decision, operating-system handle, or process identity
semantics.

### Stage 3.2 application invariants

The pure V0 topology constructor/validator accepts one `CraxiiPrincipal`, requires exactly one `Conversation`, requires its ID to equal `primary_conversation_id`, requires matching Craxii ownership, and requires exactly one matching default `WorkspaceIdentity` with the same Craxii ownership. It uses no persistence, global singleton, or startup coupling.

The pure V0 conversational input constructor/validator requires exactly one `WorkItemInput`, relationship `trigger`, ordinal one, and matching `WorkId`. It rejects every reserved relationship and does not yet verify journal-event semantic type.

### One-message-one-work rule

For an authenticated, nonduplicate conversational message command:

```text
one accepted command
  -> one user Message
  -> one message.accepted event
  -> one WorkItem in queued state
  -> one WorkItemInput relationship=trigger
  -> one work.queued event
```

All arrows commit in one SQLite transaction.

Exceptions:

- An exact idempotent retransmission returns the existing message/work IDs.
- `cancel` is a control command and creates no conversational work.
- Recovery events and runtime events are system transitions, not user messages.
- Internal test fixtures may create nonconversational work only behind test-only interfaces.

An ordinary follow-up always creates a separate work item in V0, even if it begins with “also,” “and,” or “one more thing.” There is no natural-language steering classifier.

## Work-item state machine

### States

| State | Terminal | Meaning |
| --- | ---: | --- |
| `queued` | No | Durable and eligible for FIFO claim when no earlier active work exists. |
| `running` | No | Owned by a live runtime between external waits or while processing results. |
| `waiting_on_model` | No | A persisted model attempt has begun and the runtime awaits provider completion. |
| `waiting_on_tool` | No | A tool execution is being validated, dispatched, or observed. |
| `cancel_requested` | No | Cancellation is durable; the runtime is stopping future work and cleaning up. |
| `completed` | Yes | A terminal assistant message or refusal outcome committed. |
| `failed` | Yes | Craxii observed a definite unrecoverable work failure before completion. |
| `cancelled` | Yes | Cancellation completed and all owned external activity was stopped or definitively absent. |
| `interrupted` | Yes | Runtime ownership was lost or an external attempt has an ambiguous outcome. |

`outcome_unknown` is intentionally not a work-item state. It is a terminal classification for an individual execution attempt whose side effect may have occurred. The containing work item becomes `interrupted` so the scheduler can treat it as terminal while preserving the more precise attempt evidence.

### Legal transitions

```text
queued -------------------------------> running
  |                                       |
  +--------------------------------------> cancelled
                                          |
running <--> waiting_on_model             |
running <--> waiting_on_tool              |
   |              |                       |
   +--------------+-----> cancel_requested+-----> cancelled
   |                                      |
   +--------------------------------------> completed
   +--------------------------------------> failed
   +--------------------------------------> interrupted

waiting_on_model/tool -------------------> interrupted
cancel_requested ------------------------> interrupted
```

Every transition:

- validates the expected prior state and `state_version`;
- updates the work projection;
- increments `state_version`;
- appends the corresponding journal event;
- commits atomically;
- publishes the durable client event only after commit.

Terminal work never returns to a nonterminal state. Retrying or continuing terminal work later creates a new work item correlated to the old one; V0 exposes no automatic command for this.

### Transition guards

- `queued -> running` requires no other active work in the same conversation and must be the smallest queued conversation ordinal.
- `running -> waiting_on_model` requires a persisted model invocation intent owned by the same runtime.
- `running -> waiting_on_tool` requires a persisted tool request or dispatch intent.
- `waiting_on_model -> running` requires an observed terminal provider result or a classified provider failure.
- `waiting_on_tool -> running` requires an observed terminal tool result safe to send to the model.
- Any terminal transition clears `current_model_invocation_id`, `current_tool_execution_id`, and live runtime ownership.
- `cancelled` requires cleanup to be confirmed. If cleanup cannot be confirmed, use `interrupted`.
- `completed` requires an assistant message and `assistant.message_committed` event in the same transaction.

### Stage 4 frozen lifecycle contract

Stage 4 freezes the exact durable lifecycle vocabularies below. Existing literals are
compatibility-sensitive and MUST NOT be renamed without an architecture change:

- Work states: `queued`, `running`, `waiting_on_model`, `waiting_on_tool`,
  `cancel_requested`, `completed`, `failed`, `cancelled`, and `interrupted`.
- Model invocation states: `requesting`, `streaming`, `completed`, `failed`,
  `cancelled_locally`, and `provider_outcome_unknown`.
- Tool execution states: `requested`, `dispatching`, `completed`,
  `interrupted_before_dispatch`, and `outcome_unknown`.
- Work completion reasons: `answered` and `refused`.
- Work cancellation reasons: `user_request` and `graceful_shutdown`.
- Work interruption reasons: `runtime_ownership_lost`, `provider_outcome_unknown`,
  `tool_interrupted_before_dispatch`, `tool_outcome_unknown`, and
  `cleanup_unconfirmed`.
- Lifecycle limits: `context`, `model_attempts`, `agent_loop_steps`, `tool_calls`,
  `model_output_items`, `tool_argument_bytes`, `model_invocation_time`, and
  `total_work_time`.
- Tool result classes: `success`, `validation_rejection`, `unknown_tool`,
  `authority_denial`, `file_error`, `process_exit`, `signal_termination`, `timeout`,
  `cancellation`, `spawn_failure`, and `cleanup_failure`.
- Cleanup statuses: `not_required`, `confirmed`, and `unconfirmed`.

The exact Work transition pairs are:

| From | To |
| --- | --- |
| `queued` | `running`, `cancelled` |
| `running` | `waiting_on_model`, `waiting_on_tool`, `cancel_requested`, `completed`, `failed`, `interrupted` |
| `waiting_on_model` | `running`, `failed`, `cancel_requested`, `interrupted` |
| `waiting_on_tool` | `running`, `cancel_requested`, `interrupted` |
| `cancel_requested` | `cancelled`, `interrupted` |
| `completed`, `failed`, `cancelled`, `interrupted` | None |

The exact Model transition pairs are:

| From | To |
| --- | --- |
| `requesting` | `streaming`, `completed`, `failed`, `cancelled_locally`, `provider_outcome_unknown` |
| `streaming` | `completed`, `failed`, `cancelled_locally`, `provider_outcome_unknown` |
| `completed`, `failed`, `cancelled_locally`, `provider_outcome_unknown` | None |

The exact Tool transition pairs are:

| From | To |
| --- | --- |
| `requested` | `dispatching`, `completed`, `interrupted_before_dispatch` |
| `dispatching` | `completed`, `outcome_unknown` |
| `completed`, `interrupted_before_dispatch`, `outcome_unknown` | None |

Lifecycle current state is an immutable validated snapshot. Only a Work snapshot carries a
`ProjectionVersion`; every actual Work transition increments it exactly once, no-op decisions do
not increment it, and overflow is `version_overflow`. A Work snapshot has one `runtime_owner` and
one exclusive `current_attempt = none | model(ModelInvocationId) | tool(ToolExecutionId)` shape:

- `queued` has neither owner nor current attempt;
- `running` has exactly one owner and no current attempt;
- `waiting_on_model` has exactly one owner and matching model attempt;
- `waiting_on_tool` has exactly one owner and matching tool attempt;
- `cancel_requested` retains exactly one owner and zero or one current attempt until cleanup is
  classified; and
- every terminal state clears owner and current attempt.

Work failure classes are `definite_normalized_error`, `provider_exhausted`,
`invalid_model_output`, and `lifecycle_limit`. Limit/failure completion requires definite external
cleanup; uncertainty instead interrupts Work. Work semantic event kinds are `work_started`,
`work_waiting_on_model`, `work_waiting_on_tool`, `work_resumed`, `work_cancel_requested`,
`work_completed`, `work_failed`, `work_cancelled`, and `work_interrupted`. Stage 7 owns their
durable journal envelopes and dotted event-type mapping.

Model `requesting -> streaming` occurs exactly once on the first valid provider event; later
deltas are ephemeral no-ops. Model completion requires a complete normalized response to be
durably observed by the owning transaction. A retry uses a new `ModelInvocationId`, the same
`LogicalInvocationId`, `WorkId`, `ContextManifestId`, and `AgentStepNo`, the next consecutive
`AttemptNo`, and `retry_of` naming the immediate terminal predecessor. No terminal attempt is
resurrected.

The narrower pre-authority Tool lifecycle identity contains only `ToolExecutionId`, `ExecutionId`,
`WorkId`, `RuntimeInstanceId`, source `ModelInvocationId`, `AgentStepNo`, and `ToolOrdinal`.
`ToolAttemptReference` becomes available after authority evidence exists; authority MUST NOT be
fabricated to create a requested lifecycle. `requested` proves external side effects absent.
`dispatching` means durable dispatch intent exists and action may have crossed the boundary.
`completed` means a terminal result is durably classifiable. After dispatch, unconfirmed cleanup
always becomes `outcome_unknown`; there is no path back to execution and no automatic tool retry.

Cancellation is a first-committed decision. Queued cancellation commits `cancelled`; active
cancellation commits `cancel_requested`; cancellation of `cancel_requested` or terminal Work is a
no-op with no event/version. `cancel_requested` blocks model progression, tool dispatch, another
agent-loop iteration, and final-answer commit. Confirmed local model-wait cancellation becomes
`cancelled_locally`; continuity loss becomes `provider_outcome_unknown` plus interrupted Work.
Requested Tool recovery becomes `interrupted_before_dispatch`; dispatched Tool cleanup uncertainty
becomes `outcome_unknown` plus interrupted Work. The first observed execution-control latch is
exactly `process_exit`, `timeout`, or `cancellation` and later observations cannot replace it.

Recovery classifications are exactly `retain_queued`, `already_terminal`,
`interrupt_active_work`, `mark_model_provider_outcome_unknown_and_interrupt`,
`mark_tool_interrupted_before_dispatch_and_interrupt`,
`mark_tool_outcome_unknown_and_interrupt`,
`reconcile_committed_tool_result_without_execution`, and `finalize_cancellation`. Contradictory
shapes are an invariant failure. Recovery never emits external dispatch/retry. A complete committed
model response owned by an old runtime still interrupts Work in V0; recovery neither fabricates an
assistant Message nor silently resumes the agent loop. Later model-visible uncertainty is only a
semantic synthetic-status requirement at this stage, never fabricated Message content.

Lifecycle conflicts are exactly `stale_state`, `stale_version`, `stale_owner`,
`wrong_current_attempt`, `illegal_transition`, `duplicate_terminal_decision`,
`duplicate_attempt_identity`, and `duplicate_attempt_number`, and explicitly project to the
existing `state_conflict` normalized error. Lifecycle invariants are exactly `invalid_state_shape`,
`missing_required_evidence`, `version_overflow`, `unclassifiable_recovery`,
`contradictory_projection`, and `impossible_terminal_shape`, and explicitly project to
`internal_invariant_error`. Neither surface contains raw content, provider, process, path, or tool
payloads.

## Scheduler semantics

### Durable queue

SQLite is the queue. Tokio notifications only wake the scheduler and may be lost without losing
work. Stage 10 performs an initial scan and a one-second fallback scan; a notification implementation
may be a no-op without affecting correctness.

For each conversation, eligible work is ordered by:

1. `conversation_work_ordinal` ascending;
2. `work_id` as a deterministic tie-breaker that should never be needed because ordinals are unique.

V0 ignores priority. The column may exist with a fixed value of zero but must not alter FIFO.

### One active item per conversation

The database MUST enforce, not merely assume, that a conversation has no more than one active work item. A partial unique index over active states or an equivalent transactional guard is required.

Active states are:

- `running`;
- `waiting_on_model`;
- `waiting_on_tool`;
- `cancel_requested`.

The in-process scheduler may also use a per-conversation mutex, but the database constraint remains the correctness guard.

### Claim algorithm

The scheduler repeatedly:

1. Queries conversations with queued work and no active work.
2. Begins a short write transaction.
3. Selects the smallest queued ordinal for one conversation.
4. Guardedly updates that row from `queued` to `running`, sets the current `runtime_instance_id`, and records `started_at`.
5. Appends `work.started` in the same transaction.
6. Commits.
7. Spawns the owned agent-loop task in the scheduler's task collection.

If the guarded update affects zero rows, another claim or cancellation won; the scheduler reloads state. It never assumes an in-memory queue entry is still valid.

The claim is one `WriteCoordinator` plus `BEGIN IMMEDIATE` transaction. It reloads and validates
the candidate state, version, owner, and exclusive current-attempt link; increments the Work state
version exactly once; and appends `work.started` caused by the immediately prior Work-stream event
with the current runtime actor/envelope. Only after commit may the
`after_work_claim_commit` failpoint run and the runner be registered/spawned. Claim and cancellation
therefore have deterministic first-committer outcomes: cancellation-first leaves no claimable row;
claim-first makes the subsequent active cancellation `cancel_requested`.

The scheduler owns its loop and every runner in a joined task collection. A registry keyed by
`WorkId` holds only runtime ownership, cancellation observation, and join association; it is never
recovery truth. Every task completion, start failure, panic, cancellation, and shutdown exit is
observed before removal. A committed claim whose runner cannot start, or whose runner exits
abnormally while Work remains active, becomes `interrupted` and is never requeued. Persistent
storage/invariant failure sets health `fatal`, stops claims, and requests controlled shutdown.

Claim admission closes through an explicit asynchronous quiescence barrier. Closing the latch
wakes the scheduler and prevents any later `claim_next_work` call; shutdown then waits for an
already-entered claim section to finish. That section begins before the candidate claim attempt and
ends only after no claim committed, or after a committed claim crossed
`after_work_claim_commit` and obtained runner-registry ownership or durable start-failure
classification. Only the barrier acknowledgment permits `runtime.stopping` to commit.

`WorkRunner` is a narrow application boundary. Stage 10 production has no implementation and MUST
NOT pretend to run an agent loop; permanent tests may install scripted runners. Scheduler hints for
new messages and active cancellation are lossy. A fallback database scan discovers both queued and
current-runtime `cancel_requested` Work, and durable state always wins.

### A second message during active work

If message N+1 arrives while work N is waiting on a command:

1. Authenticate and validate it normally.
2. Commit its message and work N+1 immediately.
3. Return `202 Accepted` with `state=queued`.
4. Publish its durable message/queue events.
5. Keep work N running.
6. Exclude work N+1's message and events from every context manifest for work N.
7. Claim work N+1 only after work N reaches any terminal state.

The user sees that the new responsibility was accepted and queued. The user is not asked to resubmit later.

### Scheduler shutdown

On graceful service shutdown, the scheduler:

1. Closes claim admission and acknowledges that any in-flight claim section is quiescent.
2. Marks readiness false.
3. Requests cancellation of owned agent-loop tasks.
4. Allows a bounded grace period for provider requests and tool children to stop.
5. Commits `cancelled` only where cleanup is confirmed.
6. Commits `interrupted` for active work that cannot finish or confirm cleanup before shutdown.
7. At the deadline, freezes join reconciliation, waits for conservative durable classification,
   aborts remaining children itself, and drains every expected cancelled, panic, or normal join
   result before clearing the registry.

On `SIGKILL`, startup recovery performs the classification instead.

The application shutdown latch is idempotent. Its first request fixes both a monotonic waiting
deadline and a persistable wall-clock deadline derived from the existing
`shutdown.grace_period_ms`; later requests do not extend it or append another stopping event. The
exact order is latch; mark nonfatal health `draining` while preserving already-fatal health;
close claim admission and await quiescence; commit runtime `running -> stopping` plus
`runtime.stopping`; run `during_graceful_shutdown`; stop heartbeat; request/reconcile owned Work
cancellation; notify runners; and join until the original deadline. At the deadline the scheduler
acknowledges a frozen reconciliation boundary, unresolved attempts/Work are durably classified,
and only then does the scheduler parent abort and drain its remaining child joins. Residual durable
classification precedes `stopped(graceful_shutdown)`. No cleanup is claimed without proof.
SIGINT/SIGTERM wiring remains at composition; no signal type crosses domain or ports.

## Cancellation semantics

### Client command

Cancellation is an idempotent HTTP command targeting `work_id`.

- Cancelling `queued` work atomically transitions it to `cancelled`; no side effect has started.
- Cancelling active work atomically transitions it to `cancel_requested`; asynchronous cleanup follows.
- Cancelling terminal work returns its existing terminal state as an idempotent no-op.
- Reusing a cancellation idempotency key with different target material is a conflict.

### Runtime checkpoints

The agent loop MUST check durable or in-memory cancellation state:

- before selecting/starting a model invocation;
- after a provider attempt returns;
- before persisting any tool dispatch intent;
- immediately before invoking the Workstation;
- while waiting on a tool or model;
- before starting another loop iteration;
- before committing a final assistant message.

The durable `cancel_requested` state wins over a late provider response. A response received after cancellation may be retained as invocation evidence but cannot cause tool dispatch or a completed answer.

### Provider cancellation

Cancelling a Reqwest future closes Craxii's local wait but may not stop work already accepted by the provider. The invocation records `cancelled_locally` or `provider_outcome_unknown` as appropriate. This ambiguity concerns provider cost/output, not workstation side effects. No partial output may trigger a tool.

### Tool cancellation

The Workstation receives `cancel_execution(execution_id)`.

For a local foreground command it MUST:

1. Send `SIGTERM` to the owned process group/cgroup.
2. Wait the configured grace interval.
3. Send `SIGKILL` to remaining members.
4. Reap the direct child.
5. Verify the execution cgroup is empty or record cleanup failure.
6. Close and join stdout/stderr drain tasks.

Only then may the tool result be recorded as cancelled and the work item become `cancelled`. If process-tree termination is unconfirmed, the tool becomes `outcome_unknown` and work becomes `interrupted`.

## Interruption and startup recovery

### Interruption definition

An interruption means Craxii lost the runtime continuity necessary to know that an active operation reached its intended terminal record. It is not equivalent to a definite failure.

Examples:

- backend process dies during a provider stream;
- backend dies after tool dispatch intent but before observed outcome commit;
- systemd grace period expires before process-tree cleanup is confirmed;
- artifact bytes were observed but their terminal database transaction did not commit;
- recovery finds a work item owned by an earlier runtime instance.

### Startup order

The backend MUST perform this order before readiness:

1. Parse and validate non-secret configuration.
2. Load secrets into redacted secret types.
3. Open SQLite with the required pragmas.
4. Acquire the single-instance startup lock.
5. Validate schema compatibility and apply approved migrations.
6. Run `PRAGMA quick_check` and required application invariants.
7. In one `BEGIN IMMEDIATE` transaction, create the new running runtime-instance row with
   `last_heartbeat_at = started_at` and append `runtime.started`; commit before recovery.
8. Enumerate every earlier running/stopping runtime while the lifetime-exclusive process lock is
   held, and inspect its nonterminal Work and model/tool attempts.
9. Classify old-runtime attempts and projections in bounded idempotent transactions per stale Work,
   then close the reconciled stale runtime as `stopped(startup_failure)`.
10. Append the current runtime's `runtime.recovery_performed` summary in one final transaction,
    including when all counts are zero.
11. Start owned heartbeat/event-delivery/scheduler tasks only after recovery commits.
12. Evaluate readiness; Stage 10 production remains `live_unready` until Stage 17 supplies the real
    `WorkRunner` and usable model/tool execution prerequisites.

If integrity checks or recovery writes fail, the process MUST NOT become ready. It must emit a redacted fatal diagnostic and require operator intervention rather than guessing.

If startup fails after the current runtime creation commit, the process preserves the original
error and best-effort marks that runtime `stopped(startup_failure)`. Failure of that cleanup remains
visible as a stale runtime to the next startup. Recovery never holds one write transaction across
all stale state and performs no provider, tool, process, or cleanup side effect.

### Recovery classification

| Found state owned by old runtime | Recovery action | Automatic external retry |
| --- | --- | ---: |
| `work=queued` | Leave queued and eligible | Not applicable |
| Active work with no current external attempt | Mark work `interrupted` | No |
| Model invocation `requesting` or `streaming` | Mark invocation `provider_outcome_unknown`; mark work interrupted; abandon draft | No |
| Tool execution `requested` but not dispatch-intent | Mark `interrupted_before_dispatch`; mark work interrupted | No |
| Tool execution `dispatching` | Mark `outcome_unknown`; mark work interrupted | Never |
| Tool execution terminal but work still waiting | Reconcile from committed terminal result only if journal/projection transaction is consistent; otherwise fail readiness | No new execution |
| Assistant message committed and work completed | Treat as completed even if client never received it | No |
| `cancel_requested` with no confirmed cleanup | Mark active attempt unknown as needed; mark work interrupted | No |

Queued work after an interrupted work item remains eligible because the interrupted item is terminal. Context for the next work MUST include an explicit synthetic status item describing unresolved `outcome_unknown` executions so the model does not assume success or failure.

For `cancel_requested`, zero current attempts with no cleanup ambiguity becomes `cancelled`.
Requesting/streaming model attempts first become `provider_outcome_unknown` and then interrupt
Work; an already unknown model attempt is preserved and Work interrupts. Other terminal model
attempts permit cancellation only when cleanup is confirmed or not required. Requested tools first
become `interrupted_before_dispatch`; dispatching tools first become `outcome_unknown` with cleanup
unconfirmed; existing pre-dispatch/unknown classifications are preserved; and the Work interrupts.
A completed tool permits cancellation only with confirmed/no-required cleanup. Cleanup uncertainty
always interrupts. Valid `cancel_requested` Work may retain zero or one exclusive current model or
tool attempt, whose runtime MUST equal Work ownership.

Recovery of stale waiting Work does not emit `work.resumed`: even an exactly completed attempt is
terminal evidence, not authority for Stage 10 to invent Stage 14/17 continuation. Stale waiting
Work is safely interrupted unless a later architecture explicitly supplies a continuation owner.
No recovery transition returns stale Work or an ambiguous attempt to `queued`; automatic retry of
`provider_outcome_unknown`, tool `outcome_unknown`, stale dispatch, interrupted non-idempotent side
effects, stale running Work, and stale waiting states is permanently forbidden.

### Recovery event

`runtime.recovery_performed` is a strict V1 payload containing only:

- `runtime_instance_id`;
- stale runtime instances observed and closed;
- queued Work retained and Work interrupted;
- model attempts marked provider-outcome-unknown or preserved terminal;
- tool attempts marked interrupted-before-dispatch, outcome-unknown, or preserved terminal;
- drafts abandoned, orphan artifacts observed, cleanup checks, and cleanup-unconfirmed counts;
- recovery duration in milliseconds;
- binary version, schema version, and recovery timestamp.

This event is product evidence. Detailed stack traces remain in tracing.

Its recovery window is journal order strictly after the current runtime's `runtime.started` and
strictly before its `runtime.recovery_performed`. Within that window, retained queued Work; Work
interruptions; newly unknown model attempts; newly pre-dispatch-interrupted or unknown tool
attempts; preserved terminal attempts; abandoned drafts; and cleanup checked/unconfirmed counts
are exactly reconstructed from journal causation plus stable attempt rows and MUST equal the
summary. Stale-runtime observed/closed counts, orphan-artifact observations, and recovery duration
are observational evidence that V3 cannot independently reconstruct after mutation; they receive
type/bound validation only, including `stale_runtimes_closed <= stale_runtimes_observed`. This
distinction MUST NOT be presented as historical proof that V3 does not contain.

The event is emitted exactly once for the current startup, is caused by that runtime's
`runtime.started`, and uses nonnegative signed-64-bit-safe counts. Recovery crash idempotency comes
from bounded transactions and terminal-state guards: committed units are not emitted again,
unfinished units converge on the next startup, and each new runtime receives its own start and
recovery events.

## Journal architecture

### Journal purpose

The journal is Craxii's append-only record of meaningful historical transitions. It answers:

- what responsibility was accepted;
- what caused it;
- which work state changed;
- which model or tool attempt was started;
- what terminal outcome was observed;
- which message was committed;
- what recovery classification occurred;
- in what durable order those facts became true.

It does not replace detailed tables for invocation usage, tool output metadata, or current work state. V0 uses append-only evidence plus operational projections rather than event-sourcing purity.

### Authority hierarchy inside SQLite

When records are consistent:

1. Journal events are authoritative for domain transitions and their durable order.
2. Immutable or terminal attempt rows are authoritative for detailed provider/tool evidence referenced by those events.
3. Mutable work, conversation, and scheduler rows are query-efficient current-state projections.
4. Tracing is diagnostic and never repairs product state.

If a projection contradicts the journal, readiness fails. V0 does not silently rebuild or rewrite production rows. Projector/reconstruction code MUST be deterministic and tested so an operator can diagnose and later implement an explicit repair.

### Append-only rule

After commit, a journal row MUST NOT be updated or deleted by application code. Corrections and supersessions are new events linked to the corrected event. V0 does not implement retention pruning.

SQLite access credentials are not a protection against root; append-only is an application and review invariant. Tests MUST reject repository methods that expose generic journal update/delete behavior.

## Journal event envelope

Every event has this conceptual envelope:

```text
JournalEvent
  journal_offset          INTEGER, global replay cursor
  event_id                UUIDv7, globally unique
  craxii_id               UUIDv7
  stream_id               typed string
  stream_seq              INTEGER, positive and contiguous per stream
  event_type              dotted stable name
  event_version           INTEGER >= 1
  conversation_id?        UUIDv7
  work_id?                UUIDv7
  causation_event_id?     UUIDv7
  correlation_id          distinct canonical UUIDv7 CorrelationId
  actor_kind              user | craxii | model | tool | runtime | client
  actor_id?               stable actor identifier
  runtime_instance_id?    UUIDv7
  payload_json            typed event payload
  payload_sha256          SHA-256 of stored payload bytes
  recorded_at             UTC timestamp
  occurred_at?            external occurrence timestamp when distinct
```

### Global offset

`journal_offset` MUST be an SQLite `INTEGER PRIMARY KEY AUTOINCREMENT` or equivalent never-reused monotonically increasing integer. It is:

- the durable client replay cursor;
- the total commit order for journal events on this single SQLite database;
- independent of timestamps and UUID ordering;
- returned as a decimal JSON integer that remains safe for Swift 64-bit integer storage.

A transaction appending multiple events receives consecutive offsets in insertion order. The command response returns the largest offset committed by that command.

Gaps are allowed if SQLite allocation behavior creates them. Clients care only about strict increase, not contiguity.

### Streams

`stream_id` groups events for one aggregate. V0 stream forms are:

```text
craxii:<craxii_id>
conversation:<conversation_id>
work:<work_id>
runtime:<runtime_instance_id>
```

Model and tool lifecycle events use the owning work stream and carry their own IDs in payload/table references. This avoids a stream explosion while keeping work replay coherent.

`stream_seq` is allocated transactionally from `stream_heads`. Never compute it using `MAX(stream_seq) + 1` outside a write transaction.

### Causation and correlation

- `causation_event_id` identifies the immediate durable event that caused this transition.
- `correlation_id` groups one logical responsibility or command across events.
- For conversational work, `correlation_id` SHOULD be the `work_id` after work creation.
- A client message acceptance may initially correlate on the pre-generated `work_id` even though the `work.queued` event follows it in the same transaction.
- Retry attempts share a `logical_invocation_id` in their detailed records; they do not reuse event IDs.

`parent_event_id` is not used because it fails to distinguish cause from grouping.

### Versioning

- `event_type` meaning is stable once released.
- `event_version` versions only that event's payload.
- Additive optional payload fields MAY retain the same version if old readers remain correct.
- Renamed fields, changed units, changed enum meaning, or new required fields require a new version.
- Readers MUST reject an unknown version for an event required to reconstruct current state.
- Readers MAY retain and skip an unknown event type only if it is explicitly declared non-state-bearing.
- Provider wire-event versions never become journal event versions; the adapter translates them.

Every stored payload is generated from a typed domain structure. `serde_json::Value` is allowed at the persistence boundary but MUST NOT be the source of state-machine decisions.

## Event taxonomy

### Required domain events

| Event type | Primary stream | State-bearing | Purpose |
| --- | --- | ---: | --- |
| `craxii.initialized` | Craxii | Yes | Creates the local V0 principal. |
| `conversation.created` | Conversation | Yes | Creates the primary conversation. |
| `message.accepted` | Conversation | Yes | Commits one user message and client identity. |
| `work.queued` | Work | Yes | Creates queued work and trigger input. |
| `work.started` | Work | Yes | Records scheduler claim/runtime ownership. |
| `work.waiting_on_model` | Work | Yes | Associates active work with an invocation attempt. |
| `work.waiting_on_tool` | Work | Yes | Associates active work with a tool execution. |
| `work.resumed` | Work | Yes | Records that an observed model/tool outcome returned control to the agent loop. |
| `work.cancel_requested` | Work | Yes | Persists a cancellation request. |
| `work.cancelled` | Work | Yes | Confirms terminal cancellation: Stage 9 first emits it for never-started queued Work; Stage 10 may emit it after confirmed active cleanup. |
| `work.completed` | Work | Yes | Commits terminal successful/refused outcome. |
| `work.failed` | Work | Yes | Records definite terminal failure. |
| `work.interrupted` | Work | Yes | Records loss of runtime continuity/unknown attempt. |
| `model.invocation_started` | Work | Yes | Persists outbound inference intent before network action. |
| `model.invocation_streaming` | Work | Yes | Records the first durable requesting-to-streaming observation without provider wire data. |
| `model.invocation_completed` | Work | Yes | References normalized ordered output and usage. |
| `model.invocation_failed` | Work | Yes | Records a classified observed provider failure. |
| `model.invocation_interrupted` | Work | Yes | Records ambiguous or abandoned provider attempt. |
| `tool.execution_requested` | Work | Yes | Persists model call and validated execution record before policy/dispatch. |
| `tool.execution_dispatching` | Work | Yes | Persists side-effect intent immediately before Workstation invocation. |
| `tool.execution_completed` | Work | Yes | References an observed terminal structured outcome. |
| `tool.execution_interrupted_before_dispatch` | Work | Yes | Terminalizes a requested execution after proving that no dispatch side effect occurred. |
| `tool.execution_outcome_unknown` | Work | Yes | Records ambiguous side effect after interruption/cleanup failure. |
| `assistant.message_committed` | Conversation | Yes | Commits user-visible assistant content. |
| `artifact.recorded` | Work | No for work state | Records canonical evidence artifact metadata. |
| `runtime.started` | Runtime | Yes | Identifies a backend process lifetime. |
| `runtime.recovery_performed` | Runtime | Yes | Summarizes startup classification. |
| `runtime.stopping` | Runtime | No | Records graceful shutdown intent when possible. |

The runtime kinds retain payload version 1 but do not share a generic DTO. `runtime.started`
contains exactly `runtime_instance_id`, `craxii_id`, `workstation_id`,
`workstation_generation`, diagnostic `linux_boot_id` and `process_id`, `binary_version`,
`git_revision`, `schema_version`, and `started_at`. `runtime.recovery_performed` contains exactly
the bounded recovery facts listed above. `runtime.stopping` contains exactly
`runtime_instance_id`, `shutdown_requested_at`, reason `graceful_shutdown`, `grace_deadline`,
`active_work_count`, and `active_task_count`. It is atomically appended with
`running -> stopping`; repeated shutdown requests append nothing and retain the first deadline.
There is no runtime-stopped journal kind. Startup/projector validation requires start payload/row
equality, coherent stopping history, exact recovery-window equality for journal/row-derivable
counters, and bounded validation for explicitly observational counters; it never repairs one
representation from the other.

### What is not a journal event

The following normally belong in tracing or detailed rows:

- every token delta;
- every stdout chunk;
- every SQL query;
- WebSocket ping/pong;
- provider TCP retries before an invocation attempt;
- UI rendering state;
- model-selection candidate scoring details beyond the selected reason snapshot;
- duplicate client submissions that produce no new domain transition;
- health checks;
- cache hits.

The journal records semantically meaningful boundaries, not a packet trace.

## Current-state projections

### Projection rules

- Every state-bearing event has a corresponding projection mutation in the same transaction.
- Projection rows carry `state_version` or equivalent optimistic version where concurrent commands can race.
- A transition uses `UPDATE ... WHERE state = expected AND state_version = expected_version` and verifies exactly one row changed.
- Terminal attempt data is immutable except for explicitly nonsemantic operational annotations such as post-hoc cost calculation.
- Projection queries MUST never infer terminal success merely from missing active rows.

### Deterministic reconstruction

The codebase MUST contain a pure or side-effect-free projector that can consume ordered domain events and derive at least:

- conversation message order;
- work-item lifecycle;
- current cancellation/interruption status;
- links to invocation/tool/artifact records;
- unresolved outcome-unknown warnings.

V0 startup reads current-state tables after integrity verification for speed. Tests replay the journal into an empty projection and compare the resulting domain state with committed projections. This is the guard that keeps the journal useful when memory and search projections arrive.

## SQLite schema shape

The following schema is normative in entities, relationships, uniqueness, and state constraints. Migration SQL may use different names only with an architecture update.

### Stage 6 canonical storage contract

Stage 6 is SQLx migration `0001`, version `1`, description `core durable schema`. It creates exactly
nine product tables: `craxii_principals`, `workstations`, `workspaces`, `conversations`,
`runtime_instances`, `client_devices`, `work_items`, `messages`, and `client_commands`. All nine use
`STRICT, WITHOUT ROWID`. SQLx migration metadata remains authoritative; `PRAGMA user_version`
remains zero, there is no Craxii schema-version table, migrations are forward-only, and Stage 6
inserts no product rows.

Every UUID column stores a lowercase canonical hyphenated UUIDv7 as `TEXT` and SQL checks its exact
36-character hyphen/version/variant/lowercase-hex shape. Every timestamp stores canonical UTC RFC
3339 text with exactly six fractional digits and a trailing `Z` (27 bytes); SQL checks the shape and
the Rust codec performs full calendar validation. Every SHA-256 value is 64 lowercase hexadecimal
characters. JSON is `TEXT` guarded by `json_valid` and decoded through versioned adapter-private
DTOs that deny unknown fields. All foreign keys use `ON UPDATE RESTRICT ON DELETE RESTRICT`; V0 has
no cascade, set-null, or physical-deletion behavior.

The exact runtime states are `running`, `stopping`, and `stopped`; exact stop reasons are
`graceful_shutdown` and `startup_failure`. A stopping row has no `stopped_at` and may have no reason
or `graceful_shutdown`; `startup_failure` is terminal-only. A stopped row requires both terminal
fields, and a running row permits neither. Exact client command types are `message` and `cancel`.

Work terminal-reason codes are exactly `answered`, `refused`, `definite_normalized_error`,
`provider_exhausted`, `invalid_model_output`, `lifecycle_limit`, `user_request`,
`graceful_shutdown`, `runtime_ownership_lost`, `provider_outcome_unknown`,
`tool_interrupted_before_dispatch`, `tool_outcome_unknown`, and `cleanup_unconfirmed`.
`cancellation_reason_code` is exactly `user_request` or `graceful_shutdown` while cancellation is
active. SQL enforces the complete Stage 4 owner/current-attempt/timestamp/cancellation/terminal
shape. Direct queued cancellation remains legal with `started_at = NULL`.

Message `content_json` V1 has exactly this storage shape, with one or more blocks and exact,
unnormalized UTF-8 text:

```json
{
  "version": 1,
  "blocks": [
    {
      "type": "text",
      "text": "..."
    }
  ]
}
```

The JSON representation is never hash input. Decode reconstructs `MessageContent`, regenerates the
Stage 3 canonical binary bytes, recomputes SHA-256, and requires equality with `content_sha256`.
`ix_messages_conversation` supports membership lookup only: canonical message ordering remains
journal-derived. Neither timestamp order nor UUIDv7 order is a message-order contract.

For a valid schema-version-1 database, startup validates the SQLx metadata table, the exact
migration-version set and embedded 48-byte SQLx checksum, `user_version = 0`, exact product table
and named-index sets, absence of triggers/views/unexpected objects, and a deterministic structural
manifest containing normalized `sqlite_schema` SQL plus `table_xinfo`, `foreign_key_list`,
`index_list`, `index_xinfo`, strictness, and without-rowid state. Drift, gaps, dirty or malformed
metadata, and checksum mismatch are inconsistent and are never repaired. A valid contiguous
migration history above version 1 is newer schema.

Stage 7 owns `work_item_inputs`, journal tables, projection reconstruction, and initial principal/
workstation/workspace/conversation bootstrap. Stage 8 owns model/tool/context/artifact/authority/
evidence tables. Consequently `current_model_invocation_id` and `current_tool_execution_id` have no
Stage 6 foreign key. Stage 9 owns device authentication and command idempotency behavior; Stage 10
owns runtime rows, heartbeats, scheduling, shutdown transitions, and recovery execution. Runtime
startup consistency additionally requires: queued Work has no owner; every active Work has an
existing runtime owner; a current attempt has the same runtime as its Work; the exclusive attempt
link agrees with `waiting_on_model`, `waiting_on_tool`, or valid `cancel_requested`; terminal Work
has no owner/link; and a conversation has at most one active Work. Contradiction fails closed.

### Stage 7 canonical journal and bootstrap contract

Stage 7 is SQLx migration `0002`, version `2`, description `journal and work inputs`. The maximum
supported schema version is `2`. A valid version-1 database retains its frozen Stage 6 structural
fingerprint and migrates forward through `0002`; a fresh database applies `0001` then `0002`.
Migration `0002` creates exactly `journal_events`, `stream_heads`, and `work_item_inputs`. It creates
no bootstrap metadata/checkpoint table, projector checkpoint, trigger, view, hash chain, public
visibility column, or Stage 8-or-later table. Migration-only state has zero product rows.

`journal_events.journal_offset` is the global positive SQLite
`INTEGER PRIMARY KEY AUTOINCREMENT` cursor. `event_id` is the event's canonical UUIDv7 identity,
and `stream_seq` is its positive contiguous position within one typed aggregate stream. These three
values are distinct contracts. Committed global cursors strictly increase, but gaps are legal and
MUST NOT be treated as corruption. Streams have exactly one of these canonical forms:
`craxii:<uuidv7>`, `conversation:<uuidv7>`, `work:<uuidv7>`, or `runtime:<uuidv7>`. Lowercase
prefixes, exactly one colon, and a canonical lowercase UUIDv7 are required. `stream_heads` allocates
sequence numbers inside the caller's existing `BEGIN IMMEDIATE` write transaction using one
`INSERT ... ON CONFLICT ... DO UPDATE ... RETURNING` statement. The first sequence is one; later
sequences are the previous value plus one; overflow fails. `SELECT MAX(...) + 1`, allocator-owned
transactions, and allocator commits are forbidden.

The exact stored journal envelope is `journal_offset`, `event_id`, `craxii_id`, `stream_id`,
`stream_seq`, `event_type`, `event_version`, optional `conversation_id`, optional `work_id`, optional
`causation_event_id`, `correlation_id`, `actor_kind`, optional `actor_id`, optional
`runtime_instance_id`, `payload_json`, `payload_sha256`, `recorded_at`, and optional `occurred_at`.
There is no persisted visibility or public-payload field. `actor_kind` is exactly `user`, `craxii`,
`model`, `tool`, `runtime`, or `client`; a present actor ID is a typed canonical UUIDv7, and
event-specific validation binds it to the payload identity where the contract is exact. Causation
names one already-observed lower-offset event and never self or forward; correlation groups only
the event families whose contract requires continuity. The two bootstrap events share one
correlation, and `conversation.created` is caused by `craxii.initialized`.

Payload V1 is an adapter-private, unknown-field-denying object encoding of a typed domain payload.
The envelope alone carries event type and version. `payload_sha256` is lowercase SHA-256 of the
exact UTF-8 bytes stored in `payload_json`; load recomputes and compares that digest before typed
decode and never hashes reserialized data. Payloads are JSON objects no larger than 256 KiB. An
event type retains stable meaning, while incompatible required-field, unit, or enum changes require
a new event version. Every initial registered kind supports only version one. An unknown type or
unsupported version needed for state fails closed. A future evidence-only version may be retained
as a structured warning only after a later architecture revision explicitly declares it skippable;
no current unknown is silently skipped.

Every Work-lifecycle V1 payload carries the Work ID, from/to states, expected projection version,
expected runtime owner/current attempt/cancellation reason, resulting projection version, resulting
runtime owner/current attempt/cancellation and terminal classifications, and transition timestamp.
Replay compares every expected fact with the reconstructed current projection before applying the
shared Stage 4 legal-transition rule; stream sequence is independent of projection version.

The exact 28-kind registry is the Required domain events table above. Its typed registry also
freezes current version, state-bearing classification, primary stream family, first-emitting stage,
and internal public-candidate metadata without defining Stage 11 wire payloads. Stage 7 production
code emits only `craxii.initialized` and `conversation.created`. Model, tool, context, artifact,
runtime, command, scheduler, recovery, HTTP, WebSocket, and public replay behavior remain owned by
Stages 8, 9, 10, and 11 as assigned elsewhere in this document.

`work_item_inputs` stores `(work_id, input_event_id, relationship, ordinal_within_work,
attached_at, attached_by_actor)`, with primary key `(work_id, input_event_id)` and a unique
`(work_id, ordinal_within_work)` access path. Relationships are exactly `trigger`, `steering`,
`supplemental`, `scheduled_trigger`, `external_trigger`, and `recovery_instruction`; input actors
are exactly `user`, `craxii`, `system`, and `recovery`. It has no `message_id`. The V0 conversational
causal-isolation rule is exactly one user-attached `trigger` at ordinal one whose input is the
same-conversation `message.accepted` event with the Work correlation. Later conversation messages
are never implicit Work inputs. Canonical conversation message order includes only
`message.accepted` and `assistant.message_committed` and follows their conversation `stream_seq`,
never timestamp, UUID order, or cross-stream global-offset relationships.

The Stage 7 projector lives in the application layer and consumes trusted typed events in global
offset order. It is pure and deterministic: no SQLx, I/O, clock, randomness, logging dependency,
global mutable state, or persistence mutation. It reuses the Stage 4 Work transition matrix and
reconstructs root initialization, the primary conversation, canonical message order, Work creation
and lifecycle/version/current-attempt state, and registered model/tool/artifact/runtime references
without inventing Stage 8 rows. Duplicate event identity, non-increasing global offset, per-stream
sequence gaps, missing or forward causation, required correlation mismatch, stream/link/actor
mismatch, illegal or stale Work transition, terminal resurrection, or duplicate root topology
fails closed. Global offset gaps alone do not.

Startup compares journal reconstruction with persisted state inside one bounded deferred read
snapshot. The journal is authoritative for transition/order evidence, immutable rows for detailed
canonical evidence bytes, and mutable rows for current truth. Any contradiction fails readiness;
Stage 7 performs no automatic journal rewrite, projection rebuild, or repair. Every stream with
events has exactly one head, and each head agrees with a sequence starting at one whose count,
maximum, and recorded head are equal. Append and input insertion primitives are adapter-private,
require the caller's existing write transaction, return committed positions/receipts only after
the owning transaction commits, and expose no update/delete/raw-JSON/generic-CRUD path. Append-only
enforcement is by port shape, adapter visibility, startup invariants, and permanent repository
tests; the schema deliberately has zero mutation triggers.

On the first runtime bootstrap, candidate `CraxiiId`, `WorkstationId`, `WorkspaceId`,
`ConversationId`, two event IDs, and one correlation ID are generated before the write transaction.
One `BEGIN IMMEDIATE` transaction inserts exactly one active principal (initially with both roots
null), one local workstation, one active default workspace, and one active primary conversation;
sets both principal roots together; then appends `craxii.initialized` on the Craxii stream and
`conversation.created` on the conversation stream, each at stream sequence one and in that global
order. Both use actor `craxii` with the principal ID. Principal defaults are display name `Craxii`,
owner label `local-owner`, architecture revision `V0.0.01`, and schema revision `2`; the conversation
starts at next work ordinal one and state version one. No runtime, device, Work, message, command,
or input row is created. The initialization payload freezes principal/root IDs, revision facts,
workstation generation/architecture/OS fact, capability digest, logical workspace name/root, and
creation time; `conversation.created` freezes the complete initial conversation projection.

The workstation observation is captured before the transaction from validated configuration and
portable runtime facts only. V0 records a local workstation, the configured initial generation,
configured shell/capability bounds and logical workspace, runtime architecture and OS-family fact,
and provider `unclassified`; provider instance/image/provisioning fields remain null. It does not
query cloud metadata, use hostname/PID/state-root as identity, fabricate execution capabilities, or
create a runtime. Stage 7 leaves `last_seen_at` unchanged on reopen. A complete reopen loads the
persisted IDs, appends nothing, changes no generation/version/ordinal/root link, and requires the
current observation/configuration to agree. Empty state bootstraps; any nonempty partial,
contradictory, duplicated, or mismatched root/event/head state fails closed without repair.

The `capabilities_sha256` value in `craxii.initialized` is the immutable digest of the capability
snapshot observed when that initialization event was created. A later controlled refresh of the
current `workstations.capabilities_json`, `workstations.last_seen_at`, or
`workspaces.local_resolved_root` MUST NOT rewrite or reinterpret that historical event. Durable
workstation OS evidence remains the normalized platform-family fact supplied by portable runtime
constants; exact Ubuntu 24.04 verification is a target-environment assertion and is not fabricated
on a non-Ubuntu development host.

The persistence port is split into intent-specific capability traits. Stage 7's SQLite facade
implements only bootstrap identity, snapshot loading, and consistency verification; command,
scheduler, model, tool, completion, replay, and recovery capabilities remain independently
expressible and unimplemented until their owning stages. No trait exposes SQLx, generic CRUD,
transactions, callbacks, unsupported methods, or panic placeholders. After migration/lock,
startup gathers the observation, constructs that facade, loads or creates identity, performs pure
consistency replay, loads the immutable bootstrap snapshot into `ApplicationShell`, and remains
`live_unready`. It creates no runtime, scheduler, provider/tool work, recovery action, or readiness
transition.

### Stage 8 canonical evidence-attempt and artifact contract

Stage 8 is SQLx migration `0003`, version `3`, description `context model tool artifacts`. The
maximum supported schema version is `3`. A valid version-1 or version-2 database retains its frozen
structural fingerprint and migrates forward through the remaining immutable migrations; a fresh
database applies `0001`, `0002`, and `0003` in order. Migration `0003` creates exactly five product
tables: `context_manifests`, `context_manifest_sources`, `model_invocations`, `tool_executions`, and
`artifacts`. All five use `STRICT, WITHOUT ROWID`. It creates no rows, artifact bytes or directories,
trigger, view, custom version table, down migration, chunk/output/retention table, provider
conversation table, or Stage 9-or-later table. `PRAGMA user_version` remains zero.

Every context manifest is an immutable, successfully assembled request fact. A context-limit
failure creates no manifest. Provider/model identity is stored as the exact neutral tuple
`model_target_id`, `provider_id`, `provider_model_id`, positive `target_configuration_version`, and
the versioned `model_capabilities_json` snapshot defined by `ProviderModelReference`; credentials,
account secrets, authorization headers, pricing data, provider SDK values, and provider wire
requests are forbidden. Utilization is the deterministic ceiling basis-point value of
`(estimated_input_tokens + reserved_output_tokens) / context_window_tokens`, bounded from zero
through 10,000; SQLite `REAL` is not canonical evidence. Capability decode reconstructs the
provider-neutral domain snapshot and rejects a nonpositive context window or an output limit above
that window.

Context sources are immutable and ordered only by stored positive `position`. `source_kind` uses
exactly `system_instruction`, `developer_instruction`, `workstation_capability_summary`,
`workspace_identity`, `tool_definition`, `user_message`, `active_trigger`, `assistant_message`,
`completed_model_output`, `observed_tool_result`, `artifact_content`, `synthetic_failure`,
`synthetic_interruption`, `synthetic_outcome_unknown`, `synthetic_draft_status`, or
`provider_native_continuation`. Exactly one identity family is present: a journal event, an
artifact, or a typed source record. Typed record kinds are exactly `instruction_version`,
`workstation`, `workspace`, `tool_definition`, `message`, `model_invocation`, `tool_execution`, or
`work`; the adapter validates the corresponding UUID or bounded version/definition identifier.
This taxonomy preserves future Stage 16 inputs without introducing summary memory, semantic
retrieval, selection, rendering, or token-budget behavior.

One model row is one provider attempt. Attempt and agent-step numbers are positive. Attempt one has
no predecessor; every later attempt names exactly the immediately preceding terminal attempt with
the same logical invocation, Work, context manifest, and agent step. Retry branching and gaps are
forbidden. Database uniqueness enforces the local shape and application/startup verification
enforces the cross-row chain. Model states remain exactly `requesting`, `streaming`, `completed`,
`failed`, `cancelled_locally`, and `provider_outcome_unknown`. Terminal rows are immutable through
port shape. `model.invocation_streaming` version 1 is a Work-stream state-bearing event containing
only stable provider-neutral model facts; no provider body is journaled.

One tool row is one explicit call and has no infrastructure retry. States remain exactly
`requested`, `dispatching`, `completed`, `interrupted_before_dispatch`, and `outcome_unknown`, with
the Stage 4 transition matrix unchanged. `tool.execution_interrupted_before_dispatch` version 1 is
a Work-stream state-bearing event and is never represented as outcome unknown. `requested` requires
one concrete effective requested logical current working directory. A caller that omits an
explicit directory resolves the Work/workspace default through `LogicalPathReference` before Stage
8 persistence; Stage 8 never persists or later reinterprets a null requested directory.
`resolved_cwd` remains null until dispatch intent. `requested` requires no authority decision.
`dispatching` requires a complete allow snapshot, effective privilege,
resolved working-directory evidence, timeout/output policy, and committed dispatch intent before
Stage 14 may perform any workstation, process, or filesystem side effect. Authority JSON is the
versioned adapter-private encoding of decision `allow|deny`, the existing privilege literal,
policy `v0-development-workstation`, and exactly one reason code from `registered_tool`,
`policy_denied`, `malformed_request`, `limit_exceeded`, `work_cancelled`, or `scope_denied`.
A denial cannot claim effective `admin` privilege.
A definite `completed` result before dispatch may retain an optional `deny` snapshot; it has no
dispatch timestamp, effective dispatch privilege, resolved path, process result, or output bytes.
`interrupted_before_dispatch` retains no authority snapshot because evaluation did not authorize or
cross the dispatch boundary.

For stdout and stderr independently, observed bytes are all drained bytes, captured bytes are the
stored prefix under the hard capture limit, returned-inline bytes are the bounded portion exposed
to later model/result semantics, and omitted bytes equal observed minus returned-inline. Therefore
`observed >= captured >= returned_inline`, and these quantities are never collapsed. Artifact
SHA-256 identifies captured bytes only. The tool row records the per-stream counts; no raw process
bytes enter SQLite or journal JSON.

`ArtifactId` is a semantic provenance identity. Physical storage is content addressed by
`sha256/<first-two-lowercase-hex>/<64-lowercase-hex>` and may be shared by multiple Artifact IDs.
Consequently neither storage key nor `(sha256, captured_byte_count, backend)` is unique. To avoid a
circular foreign-key graph, `artifacts` physically references the Craxii principal and optional
producing Work, then stores a typed `producer_kind` plus canonical producer UUID without a reverse
model/tool foreign key. Model/tool/context artifact references retain physical foreign keys to
`artifacts`; startup validates that each typed producer exists and agrees with the producing Work.

The dependency-neutral `ArtifactStore` exposes bounded capture, finalization, verified read,
verification, and orphan classification without SQLx, absolute paths, or local filesystem types.
The local adapter owns `<artifact_root>/tmp` and `<artifact_root>/sha256`, uses modes 0700 for
directories and 0600 for files, rejects symlinks/non-regular files/permissive modes/unsafe
hardlinks/cross-filesystem publication/path traversal, and uses exclusive no-follow creation.
Finalization order is exact: generate Artifact ID; exclusively create `tmp/<artifact-id>.partial`;
stream, bound, hash, and count; flush; `sync_all` the temporary file; create and verify the digest
shard; publish without replacement using `renameat2(RENAME_NOREPLACE)` on Linux or
`renamex_np(RENAME_EXCL)` on macOS; sync required containing directories; reopen and fully verify
regular-file type, 0600 mode, link count one, size, and digest; then return a finalized descriptor.
Only after that return may an owning SQLite transaction insert metadata or a reference. An existing
canonical target is reused only after the same full verification; it is never overwritten.
`FinalizedArtifact` is a sealed durable-publication capability: its fields are private and only the
local artifact adapter may construct it after this sequence. SQLite may reconstruct a distinct
read-verification reference from committed metadata, but cannot mint a finalized descriptor.

Every semantic artifact metadata row committed for context/model/tool evidence appends exactly one
`artifact.recorded` event on the owning Work stream. When a transaction introduces artifacts, each
artifact event precedes the terminal attempt event that references it. Model begin ordering is
`artifact.recorded` when applicable, `model.invocation_started`, then caused
`work.waiting_on_model`; streaming appends `model.invocation_streaming`; model terminal ordering is
artifact events, terminal model event, then a caused legal Work event when supplied. Tool request
ordering is `tool.execution_requested` then caused `work.waiting_on_tool`; dispatch appends
`tool.execution_dispatching`; terminal ordering is artifact events, the exact tool terminal event,
then a caused legal Work event. There is no context-manifest event.

`work_items` is not rebuilt and its current model/tool columns gain no physical reverse foreign
key. Named Stage 8 transactions maintain application-enforced reverse links atomically. Startup
requires waiting-on-model to name exactly one matching nonterminal model attempt, waiting-on-tool
to name exactly one matching nonterminal tool execution, all other Work states to have neither,
matching Work/runtime ownership where required, model/tool XOR, terminal attempts absent from
current links, valid retry chains, and journal/detail/projector agreement.

Named Stage 8 persistence operations receive complete prebuilt identities, facts, event IDs,
timestamps, and already finalized artifact descriptors. They implement begin/stream/terminal model
transactions and request/dispatch-intent/terminal tool transactions only; they perform no provider,
workstation, process, filesystem-workspace, registry, policy-evaluation, context-assembly, or agent
loop behavior. A transaction may apply a caller-supplied legal Stage 4 Work transition, but after
commit a current model/tool link never points to a terminal attempt. `CompletionStateStore` and the
assistant-message/final-work transaction belong to Stage 17, not Stage 8.

Every named transaction validates the complete artifact set before inserting any artifact row or
event. The distinct supplied Artifact IDs must equal the distinct IDs referenced by that owning
model, tool, manifest, and context-source request: missing, extra, or duplicate descriptors are
rejected. Reuse is rejected across incompatible semantic roles; the sole model-begin reuse allowed
is an explicitly identical attempt-one invocation request and manifest rendered request with equal
ID, digest, count, producer, and Work facts. Each descriptor is cross-checked against its referenced
digest, captured/observed counts, truncation, producer identity, and producing Work. Existing
context-source artifacts are accepted only as already-valid durable same-Work metadata with the
same digest. Only after the whole set passes may the SQLite transaction insert artifact metadata
and events, owning rows, Work/journal mutations, and commit once; the artifact helper never commits.

Artifact directories are initialized only after V3 SQLite migration/integrity succeeds. Stage 8
startup order is configuration validation; SQLite lock/runtime/migration/integrity; artifact-root
initialization and hardening; Stage 7 identity load/bootstrap; journal/application consistency;
Stage 8 relational attempt consistency; full verification of every distinct referenced artifact;
orphan scan/report; bootstrap snapshot; remain `live_unready`. Missing, corrupt, unsafe, or
metadata-inconsistent referenced bytes are fatal and never repaired. Temp partials and valid final
objects with no metadata reference are nonfatal orphans. Ordinary startup never deletes them.
Explicit maintenance may consider only an unreferenced canonical final object at least 24 hours old
and must recheck SQLite immediately before deletion; Stage 8 provides classification and pure
eligibility only, with no scheduled or automatic deletion.

Stage ownership remains narrow: Stage 9 owns authentication, device-token verification, message and
cancel commands, command idempotency, and direct terminal cancellation of queued Work; Stage 10
owns runtime lifecycle, scheduling, recovery execution, readiness, and completion of only active
cancel-requested cleanup paths; Stage 14 owns registry resolution, authority evaluation, and actual
tool side effects; Stage 15 owns provider ports/adapters and canonical provider semantics; Stage 16
owns context eligibility, selection, rendering, and token budgeting; Stage 17 owns provider calls,
the agent loop, and terminal assistant completion.

### Stage 9 canonical authentication and command contract

Stage 9 adds no migration. SQLx migrations remain exactly `0001` through `0003`, the supported
schema ceiling remains `3`, and the existing 17 tables, 40 named indexes, zero triggers/views, and
28 journal event kinds are unchanged. Stage 9 implements behavior over the V3 `client_devices`,
`client_commands`, message, Work, input, conversation, journal, and stream-head schema. It changes
neither an existing event payload meaning nor an event payload version.

#### Device bearer credentials and offline administration

A V0 bearer token is exactly 32 bytes of operating-system CSPRNG output encoded locally as 64
lowercase hexadecimal ASCII characters. Accepted input is exactly `[0-9a-f]{64}`: uppercase,
prefixes, whitespace, and every other length or character are invalid. The token is opaque and
contains neither a `DeviceId` nor a version. `getrandom` `0.4` is the direct dependency used only to
fill the exact 32-byte production token buffer; V0 adds no fallback RNG, `rand`, JWT, base64, HMAC,
password hashing, `zeroize`, or `subtle` dependency.

The stored token hash is `SHA-256` of the exact accepted 64 ASCII bearer bytes, encoded as lowercase
hexadecimal in `client_devices.token_hash`. Authentication hashes those accepted textual bytes; it
does not hex-decode and hash the original 32 random bytes. The production `BearerToken` keeps its
text private, has redacted `Debug`, has no revealing `Display`, is not serializable, and exposes
only narrow hashing and one-time issuance operations. V0 makes no memory-zeroization claim. A raw
token MUST NOT enter SQLite, journal data, artifacts, command hashes, logs/tracing, errors/panics,
child environments, or diagnostics.

A device display name preserves the exact validated Unicode text without case or Unicode
normalization. It is 1 through 128 UTF-8 bytes, contains no control character, and has no leading or
trailing whitespace; validation never silently trims it. Provisioning generates a new `DeviceId`,
hash, creation timestamp, null `last_seen_at`, and null `revoked_at`. Active means
`revoked_at IS NULL`. Revocation is one-way: it sets `revoked_at` only when null, and a repeated
revoke returns the existing timestamp without rewriting it. Credential rotation is replacement
based: provision a replacement device/token and then revoke the old device; V0 never overwrites
`token_hash` or mutates a display name.

Provisioning, safe listing, and revocation are offline `craxii-admin device` operations, not client
commands or public enrollment. The admin path validates configuration/state root, acquires the
same exclusive lifetime process lock as Craxii, opens/migrates/checks V3, validates the canonical
bootstrap topology and application consistency, and performs no runtime creation, readiness,
scheduler, recovery, provider, or tool work. Listing reveals only `DeviceId`, display name,
active/revoked status, and creation/last-seen/revocation timestamps. Provision commits before it
writes the raw token exactly once to a dedicated stdout result line; metadata may use stderr but
never includes the token or hash. If stdout fails after commit, the unrecoverable raw token is not
reissued because it was never stored; the operator provisions a replacement and may revoke the
unreachable credential. Device administration emits no journal event. Zero devices remains a
valid bootstrapped state, and ordinary startup never creates a device.

#### Application authentication order

`DeviceAuthenticator` receives a bearer secret at the application edge. It validates exact token
grammar, hashes the exact ASCII bytes, looks up an active device by digest, performs a reviewed
full-length XOR-accumulating comparison of the expected and returned 32-byte digests as defense in
depth, rejects a non-null `revoked_at`, and returns only an `AuthenticatedDevice` containing the
`DeviceId`. Malformed, missing, unknown, and revoked credentials collapse to one safe
`authentication_failed` result with no existence or revocation oracle. Authentication always
precedes idempotency lookup or command execution.

After successful authentication, a best-effort evidence-only `last_seen_at` touch may run outside
the command transaction. It updates only when the new canonical timestamp is later than the stored
value. Touch failure never changes authentication success or command correctness and emits no
journal event. Startup validates canonical device IDs, display names, digest/timestamps,
`last_seen_at >= created_at`, and `revoked_at >= created_at` without requiring a device row.

#### Command identity and canonical request hashes

The exact Stage 9 command kinds stored in `client_commands.command_type` are `message` and
`cancel`. An `IdempotencyKey` is a canonical lowercase UUIDv7 string, scoped by
`(DeviceId, IdempotencyKey)` across both kinds. Different devices may reuse one UUID. A message key
MUST equal its `ClientMessageId`; a cancellation key MUST equal its `ClientCommandId`. A public
message key/body mismatch is validation failure before persistence. Reusing one scoped key for a
different kind or canonical request is `idempotency_conflict`.

Command request hash encoding version 1 is independent of the database schema. Its exact byte
sequence begins with ASCII `craxii.command`, followed by the single byte `0x01`. Every following
fixed-position field is encoded as an unsigned 64-bit big-endian byte length followed by the exact
field bytes. Protocol version is the existing V0 protocol version represented by exactly eight
unsigned 64-bit big-endian bytes. Message fields, in order, are protocol version, ASCII `message`,
canonical lowercase `ConversationId` text, canonical lowercase `ClientMessageId` text, and the
exact existing `MessageContent::canonical_bytes()`. Cancellation fields, in order, are protocol
version, ASCII `cancel`, canonical lowercase `ClientCommandId` text, and canonical lowercase
`WorkId` text. `CommandRequestHash` is SHA-256 of the complete sequence.

The hash excludes `DeviceId`, a separately repeated idempotency key, bearer secret/digest,
server-generated timestamps and IDs, message `WorkId`, event IDs, transport request IDs, raw JSON,
and HTTP bytes. Consequently JSON member order/whitespace is irrelevant while exact typed Unicode
message bytes remain significant. Version 1 known vectors are permanent; a later format must retain
V1 decoding and recomputation for already committed rows.

#### Committed command-row and replay semantics

`client_commands` is insert-only in production. There is no pending row and no production update
or delete API. Row presence means a final logical result and all of its effects or accepted no-op
are durable in the same transaction. The adapter owns a strict versioned response codec rather
than persisting an HTTP framework object or arbitrary response body. On lookup, it validates the
canonical scoped key, known kind, canonical request hash, allowed status, unknown-field-denying
response DTO, positive cursor, and canonical creation time. Corrupt or contradictory stored data is
`storage_inconsistent`, never a replay.

A stored message receipt is exact V1 JSON with only `version`, `conversation_id`, `message_id`,
`work_id`, `work_ordinal`, `work_state` equal to `queued`, and `committed_cursor`; its stored status
is `202`. A stored cancellation receipt contains only `version`, `work_id`,
`resulting_work_state`, `cleanup_pending`, and `committed_cursor`. Status is `200` for direct queued
cancellation or a terminal no-op and `202` for a new/already `cancel_requested` result. A transport
may later add a duplicate convenience flag, but duplicate state is not stored in the logical DTO.

Authentication/validation failure, malformed key/body, invalid content, idempotency conflict,
unknown or unauthorized cancellation target, transient storage error, and detected inconsistency
create no command row. A new valid cancellation key targeting `cancel_requested`, `completed`,
`failed`, `cancelled`, or `interrupted` does persist its accepted stable no-op receipt. Exact replay
returns the stored logical IDs, state, status, and cursor; it never regenerates persisted IDs.

Inside the production `WriteCoordinator` and `BEGIN IMMEDIATE`, a command first loads the scoped
row. Exact kind/hash match strictly decodes and replays it; mismatch conflicts. If absent, the
transaction authorizes the V0 target, applies any semantic mutation and journal append, constructs
the stable receipt/cursor, inserts `client_commands` last, and commits once. A uniqueness loser on
an alternate connection reloads the winner and applies the same match/conflict rule. The normal
coordinator serializes duplicates so one same-request winner mutates once and different material
conflicts. Post-commit scheduler/event notifications are lossy hints and never a condition of
command durability.

The Stage 10 postcommit notification boundary has an explicit no-op fallback. A truly new message
commit reaches `after_message_transaction_commit` before its scheduler hint and caller delivery. A
truly new active cancellation commit reaches `after_cancel_requested_commit` before its
cancellation hint and caller delivery. `CommandOutcome::Committed|Replayed` distinguishes the
cases without changing persisted rows, response DTOs, or canonical hashes. Replay, queued direct
cancellation, and terminal no-op never re-trigger a new-commit crash window. The scheduler's
one-second fallback scan makes a lost or no-op notifier correct.

#### Atomic message acceptance

A new message command pre-generates candidate `MessageId`, `WorkId`, `message.accepted` and
`work.queued` `JournalEventId` values. Lost candidates after rollback are harmless. The distinct
`CorrelationId` is derived explicitly from the exact UUID bytes of `WorkId`; this is a narrowly
named conversion, not general ID interchangeability.

In one immediate transaction, after the idempotency check, the adapter loads the sole principal,
primary active conversation, and default workspace; reads conversation ordinal `N` and state
version `V`; constructs and inserts the immutable user message with authenticated
`DeviceId`/`ClientMessageId`, exact canonical content/hash, and server command timestamp; appends
`message.accepted` in the conversation stream caused by `conversation.created`; inserts one
conversational queued Work at ordinal `N`, state version `1`, priority `0`, no runtime/current
attempt/cancellation/terminal fields, and `created_at == queued_at`; inserts exactly one trigger
input at ordinal `1` attached by `user` to the acceptance event; appends `work.queued` in the Work
stream caused by `message.accepted`; guardedly advances conversation ordinal to `N+1` and version
to `V+1`; builds the receipt using the `work.queued` offset; inserts the command row last; and
commits.

The acceptance actor is `user` with actor ID the authenticated `DeviceId`; the queue actor is
`craxii` with the sole `CraxiiId`. Both events use the Work-derived correlation and retain their
existing V1 payload meaning. Work ordinal follows serialized commit order, never timestamps or
UUID order. Each Work input closes over only its own acceptance event; later accepted messages do
not become its inputs. Any failure before commit rolls back message, Work, input, events, stream
heads, conversation ordinal/version, and command row. The existing message client-identity unique
index is defense in depth: a bypassed duplicate fails closed and cannot create an orphan success
receipt.

#### Idempotent cancellation matrix

Cancellation reloads the authoritative Work in the immediate transaction and uses the existing
Stage 4 cancellation decision; it never decides from a stale caller snapshot. Any active V0 device
may cancel Work belonging to the sole principal/primary-conversation/default-workspace topology;
the creating device is not an authorization boundary. Unknown or foreign targets return
`target_not_found` with no row, mutation, or event.

For `queued`, Stage 9 is the first emitter of `work.cancelled`: it directly transitions to
`cancelled`, increments state version once, sets the exact existing `user_request` terminal reason
and command timestamp, leaves `started_at` null and all runtime/attempt facts absent, appends the
event as user/authenticated-Device actor with existing Work correlation and latest Work-stream event
causation, persists a `200`/`cleanup_pending=false` receipt whose cursor is that event offset, and
commits atomically. No external cleanup is claimed because no activity ever began.

For `running`, `waiting_on_model`, and `waiting_on_tool`, Stage 9 transitions to
`cancel_requested`, increments the version, preserves runtime and current-attempt facts, sets the
existing `user_request` request timestamp/reason, appends `work.cancel_requested` caused by the
latest Work-stream event, and persists a `202`/`cleanup_pending=true` receipt at that offset. Stage
9 does not signal a runner, cancel a provider, kill a process, mutate an attempt, clear ownership,
classify an outcome, or terminalize active Work.

For existing `cancel_requested`, no Work update or event occurs; a new valid command key persists a
`202`/`cleanup_pending=true` receipt at the positive journal high-water observed inside the same
transaction. For `completed`, `failed`, `cancelled`, and `interrupted`, no update/event occurs and a
new key persists a `200`/`cleanup_pending=false` receipt at that in-transaction high-water. A valid
Work implies at least one journal row; an absent/nonpositive head is inconsistency. Event causation
is loaded from the latest authoritative Work-stream event, never guessed from state.

No-op receipt consistency is necessarily the strongest sound V3 invariant rather than a claim of
an event that does not exist: the cursor must name an existing offset no later than current head,
the Work must exist, and the receipt state must be compatible with its current immutable terminal
state or reconstructable cancel-requested history. Event-bearing cancellation receipts must match
the exact `work.cancel_requested` or `work.cancelled` event and cursor. This limitation is the
accepted no-migration tradeoff.

Stage 10 may later emit `work.cancelled` only after confirmed cleanup of active
`cancel_requested` Work. It never re-terminalizes queued Work already cancelled by Stage 9. Stage
10 owns runner signaling and active cleanup; Stage 11 owns bearer extraction, uniform HTTP `401`,
HTTP request/response mapping, duplicate convenience fields, and WebSocket transport; Stage 17
owns provider calls, agent-loop progression, assistant-message completion, and
`CompletionStateStore`.

The Stage 2 production failpoint names `after_message_transaction_commit` and
`after_cancel_requested_commit` remain registered but have no Stage 9 callsite; Stage 10.4 owns
their activation. Stage 9 transaction tests use adapter-private test-only precommit rollback hooks.
Postcommit lost-response tests discard the returned receipt at the application/test boundary,
close/reopen the file-backed store, and prove exact replay without adding a production failpoint.

### `craxii_principals`

```sql
craxii_principals (
  craxii_id                    TEXT PRIMARY KEY,
  display_name                 TEXT NOT NULL,
  owner_label                  TEXT NOT NULL,
  lifecycle_state              TEXT NOT NULL CHECK (lifecycle_state = 'active'),
  primary_conversation_id      TEXT,
  default_workspace_id         TEXT,
  created_at                   TEXT NOT NULL,
  architecture_revision        TEXT NOT NULL,
  schema_revision              INTEGER NOT NULL CHECK (schema_revision >= 1),
  FOREIGN KEY (primary_conversation_id) REFERENCES conversations
    ON UPDATE RESTRICT ON DELETE RESTRICT,
  FOREIGN KEY (default_workspace_id) REFERENCES workspaces
    ON UPDATE RESTRICT ON DELETE RESTRICT,
  CHECK ((primary_conversation_id IS NULL) = (default_workspace_id IS NULL))
)
```

Only one row is permitted in V0 by application invariant.

### `conversations`

```sql
conversations (
  conversation_id       TEXT PRIMARY KEY,
  craxii_id              TEXT NOT NULL REFERENCES craxii_principals,
  kind                   TEXT NOT NULL CHECK (kind = 'primary'),
  lifecycle_state        TEXT NOT NULL CHECK (lifecycle_state = 'active'),
  next_work_ordinal      INTEGER NOT NULL CHECK (next_work_ordinal >= 1),
  state_version          INTEGER NOT NULL DEFAULT 1,
  created_at             TEXT NOT NULL,
  UNIQUE (craxii_id, kind)
)
```

### `messages`

```sql
messages (
  message_id             TEXT PRIMARY KEY,
  craxii_id              TEXT NOT NULL REFERENCES craxii_principals,
  conversation_id        TEXT NOT NULL REFERENCES conversations,
  role                   TEXT NOT NULL CHECK (role IN ('user','assistant','system')),
  content_json           TEXT NOT NULL,
  content_sha256         TEXT NOT NULL,
  produced_by_work_id    TEXT REFERENCES work_items,
  client_device_id       TEXT REFERENCES client_devices,
  client_message_id      TEXT,
  committed_at           TEXT NOT NULL,
  UNIQUE (client_device_id, client_message_id),
  UNIQUE (produced_by_work_id)
)
```

The unique client fields are nullable only for assistant/system messages. Application validation requires both or neither.

### `work_items`

```sql
work_items (
  work_id                       TEXT PRIMARY KEY,
  craxii_id                     TEXT NOT NULL REFERENCES craxii_principals,
  conversation_id               TEXT NOT NULL REFERENCES conversations,
  conversation_work_ordinal     INTEGER NOT NULL,
  kind                          TEXT NOT NULL CHECK (kind = 'conversational'),
  state                         TEXT NOT NULL,
  state_version                 INTEGER NOT NULL DEFAULT 1,
  priority                      INTEGER NOT NULL DEFAULT 0 CHECK (priority = 0),
  workspace_id                  TEXT NOT NULL REFERENCES workspaces,
  runtime_instance_id           TEXT REFERENCES runtime_instances,
  current_model_invocation_id   TEXT,
  current_tool_execution_id     TEXT,
  correlation_id                TEXT NOT NULL,
  created_at                    TEXT NOT NULL,
  queued_at                     TEXT NOT NULL,
  started_at                    TEXT,
  cancel_requested_at           TEXT,
  cancellation_reason_code      TEXT,
  terminal_at                   TEXT,
  terminal_reason_code          TEXT,
  terminal_detail_json          TEXT,
  UNIQUE (conversation_id, conversation_work_ordinal)
)
```

The migration MUST add check constraints for timestamp/state consistency where SQLite permits. A partial unique index MUST prevent two active rows for one conversation:

```sql
UNIQUE (conversation_id)
WHERE state IN (
  'running', 'waiting_on_model', 'waiting_on_tool', 'cancel_requested'
)
```

### `work_item_inputs`

```sql
work_item_inputs (
  work_id              TEXT NOT NULL REFERENCES work_items,
  input_event_id       TEXT NOT NULL REFERENCES journal_events(event_id),
  relationship         TEXT NOT NULL,
  ordinal_within_work  INTEGER NOT NULL CHECK (ordinal_within_work >= 1),
  attached_at          TEXT NOT NULL,
  attached_by_actor    TEXT NOT NULL,
  PRIMARY KEY (work_id, input_event_id),
  UNIQUE (work_id, ordinal_within_work)
)
```

V0 application logic permits one row with `relationship='trigger'` and ordinal 1.

### `client_devices`

```sql
client_devices (
  device_id          TEXT PRIMARY KEY,
  display_name       TEXT NOT NULL,
  token_hash         TEXT NOT NULL UNIQUE,
  created_at         TEXT NOT NULL,
  last_seen_at       TEXT,
  revoked_at         TEXT
)
```

The raw bearer token is never stored.

### `client_commands`

```sql
client_commands (
  device_id             TEXT NOT NULL REFERENCES client_devices,
  idempotency_key       TEXT NOT NULL,
  command_type          TEXT NOT NULL,
  request_hash          TEXT NOT NULL,
  response_http_status  INTEGER NOT NULL,
  response_json         TEXT NOT NULL,
  committed_cursor      INTEGER NOT NULL,
  created_at            TEXT NOT NULL,
  PRIMARY KEY (device_id, idempotency_key)
)
```

Rows appear only in the same transaction as the command's effects. There is no durable “pending command” state in V0.

### `journal_events`

```sql
journal_events (
  journal_offset       INTEGER PRIMARY KEY AUTOINCREMENT,
  event_id             TEXT NOT NULL UNIQUE,
  craxii_id            TEXT NOT NULL REFERENCES craxii_principals,
  stream_id            TEXT NOT NULL,
  stream_seq           INTEGER NOT NULL CHECK (stream_seq >= 1),
  event_type           TEXT NOT NULL,
  event_version        INTEGER NOT NULL CHECK (event_version >= 1),
  conversation_id      TEXT REFERENCES conversations,
  work_id              TEXT REFERENCES work_items,
  causation_event_id   TEXT REFERENCES journal_events(event_id),
  correlation_id       TEXT NOT NULL,
  actor_kind           TEXT NOT NULL,
  actor_id             TEXT,
  runtime_instance_id  TEXT REFERENCES runtime_instances,
  payload_json         TEXT NOT NULL,
  payload_sha256       TEXT NOT NULL,
  recorded_at          TEXT NOT NULL,
  occurred_at          TEXT,
  UNIQUE (stream_id, stream_seq)
)
```

### `stream_heads`

```sql
stream_heads (
  stream_id       TEXT PRIMARY KEY,
  last_stream_seq INTEGER NOT NULL CHECK (last_stream_seq >= 0)
)
```

### `runtime_instances`

```sql
runtime_instances (
  runtime_instance_id   TEXT PRIMARY KEY,
  craxii_id             TEXT NOT NULL REFERENCES craxii_principals,
  workstation_id        TEXT NOT NULL REFERENCES workstations,
  workstation_generation INTEGER NOT NULL,
  linux_boot_id         TEXT NOT NULL,
  process_id            INTEGER NOT NULL,
  binary_version        TEXT NOT NULL,
  git_revision          TEXT NOT NULL,
  schema_version        INTEGER NOT NULL,
  state                 TEXT NOT NULL,
  started_at            TEXT NOT NULL,
  last_heartbeat_at     TEXT,
  stopped_at            TEXT,
  stop_reason           TEXT
)
```

### `workstations` and `workspaces`

```sql
workstations (
  workstation_id       TEXT PRIMARY KEY,
  craxii_id             TEXT NOT NULL REFERENCES craxii_principals,
  kind                  TEXT NOT NULL CHECK (kind = 'local'),
  generation            INTEGER NOT NULL CHECK (generation >= 1),
  hosting_provider      TEXT NOT NULL,
  provider_instance_id  TEXT,
  provider_image_id     TEXT,
  provisioning_revision TEXT,
  architecture          TEXT NOT NULL,
  os_release            TEXT NOT NULL,
  capabilities_json     TEXT NOT NULL,
  created_at            TEXT NOT NULL,
  last_seen_at          TEXT NOT NULL
)

workspaces (
  workspace_id          TEXT PRIMARY KEY,
  craxii_id             TEXT NOT NULL REFERENCES craxii_principals,
  workstation_id        TEXT NOT NULL REFERENCES workstations,
  logical_name          TEXT NOT NULL,
  logical_root          TEXT NOT NULL,
  local_resolved_root   TEXT NOT NULL,
  lifecycle_state       TEXT NOT NULL CHECK (lifecycle_state = 'active'),
  created_at            TEXT NOT NULL,
  UNIQUE (workstation_id, logical_name)
)
```

`local_resolved_root` is an adapter detail retained for evidence. Domain and protocol logic use `workspace_id` and logical paths.

### `context_manifests`

```sql
context_manifests (
  context_manifest_id       TEXT PRIMARY KEY,
  work_id                   TEXT NOT NULL REFERENCES work_items,
  logical_invocation_id     TEXT NOT NULL,
  model_target_id           TEXT NOT NULL,
  provider_id               TEXT NOT NULL,
  provider_model_id         TEXT NOT NULL,
  target_configuration_version INTEGER NOT NULL,
  model_capabilities_json   TEXT NOT NULL,
  assembler_version         TEXT NOT NULL,
  context_policy_version    TEXT NOT NULL,
  system_prompt_fingerprint TEXT NOT NULL,
  toolset_fingerprint       TEXT NOT NULL,
  eligibility_cutoff_json   TEXT NOT NULL,
  source_count              INTEGER NOT NULL,
  canonical_byte_count      INTEGER NOT NULL,
  rendered_request_byte_count INTEGER NOT NULL,
  estimated_input_tokens    INTEGER NOT NULL,
  token_estimator_id        TEXT NOT NULL,
  context_window_tokens     INTEGER NOT NULL,
  reserved_output_tokens    INTEGER NOT NULL,
  utilization_basis_points  INTEGER NOT NULL,
  manifest_sha256           TEXT NOT NULL,
  rendered_request_sha256   TEXT NOT NULL,
  rendered_request_artifact_id TEXT REFERENCES artifacts,
  omissions_json            TEXT NOT NULL,
  created_at                TEXT NOT NULL
)
```

### `context_manifest_sources`

```sql
context_manifest_sources (
  context_manifest_id  TEXT NOT NULL REFERENCES context_manifests,
  position             INTEGER NOT NULL,
  source_kind          TEXT NOT NULL,
  event_id             TEXT REFERENCES journal_events(event_id),
  artifact_id          TEXT REFERENCES artifacts,
  source_record_kind   TEXT,
  source_record_id     TEXT,
  model_role           TEXT,
  item_class           TEXT,
  source_content_sha256 TEXT NOT NULL,
  rendered_byte_contribution INTEGER NOT NULL,
  transform_json       TEXT NOT NULL,
  PRIMARY KEY (context_manifest_id, position)
)
```

Exactly one source identity family must be present. `transform_json` records normalization, inline
projection, or synthetic status rendering. Source positions start at one, are contiguous at load,
and reconcile exactly with the immutable parent `source_count`.

### `model_invocations`

```sql
model_invocations (
  model_invocation_id       TEXT PRIMARY KEY,
  logical_invocation_id     TEXT NOT NULL,
  work_id                   TEXT NOT NULL REFERENCES work_items,
  runtime_instance_id       TEXT NOT NULL REFERENCES runtime_instances,
  context_manifest_id       TEXT NOT NULL REFERENCES context_manifests,
  agent_step_no             INTEGER NOT NULL,
  attempt_no                INTEGER NOT NULL,
  retry_of_invocation_id    TEXT REFERENCES model_invocations,
  model_target_id           TEXT NOT NULL,
  provider_id               TEXT NOT NULL,
  provider_model_id         TEXT NOT NULL,
  target_configuration_version INTEGER NOT NULL,
  model_capabilities_json   TEXT NOT NULL,
  selection_reason          TEXT NOT NULL,
  required_capabilities_json TEXT NOT NULL,
  provider_options_json     TEXT NOT NULL,
  state                     TEXT NOT NULL,
  request_sha256            TEXT NOT NULL,
  request_artifact_id       TEXT REFERENCES artifacts,
  response_sha256           TEXT,
  response_artifact_id      TEXT REFERENCES artifacts,
  normalized_output_json    TEXT,
  provider_request_id       TEXT,
  provider_response_id      TEXT,
  started_at                TEXT NOT NULL,
  first_byte_at             TEXT,
  first_output_at           TEXT,
  completed_at              TEXT,
  input_tokens              INTEGER,
  cached_input_tokens       INTEGER,
  output_tokens             INTEGER,
  reasoning_tokens          INTEGER,
  total_tokens              INTEGER,
  stop_reason               TEXT,
  tool_call_count           INTEGER,
  draft_exposed             INTEGER NOT NULL DEFAULT 0,
  normalized_error_json     TEXT,
  UNIQUE (logical_invocation_id, attempt_no),
  UNIQUE (work_id, agent_step_no, attempt_no)
)
```

Each provider attempt is a new row. `logical_invocation_id` groups retries of the same agent step and context manifest.

The schema also has a partial unique predecessor index, a partial unique one-nonterminal-model-row
per Work index, and indexes for runtime/nonterminal and context/attempt lookup. Request, capability,
requirement, options, normalized output, and normalized error JSON use bounded versioned
adapter-private DTOs that deny unknown fields. The normalized output envelope preserves ordered
provider-neutral items without making Stage 15 wire types canonical. Decode is symmetric with
encode: it reconstructs canonical IDs and existing domain types, applies the same key grammar,
bounded text/JSON and numeric relations, validates every nested item and closed literal, and rejects
duplicates or impossible cross-field shapes. Deserializing storage strings is never sufficient to
trust persisted identity or provider-neutral evidence.

Model invocation states are:

- `requesting`: intent is durable and the request may or may not have reached the provider;
- `streaming`: at least one valid provider stream event was observed;
- `completed`: one complete normalized provider response and usage record was observed;
- `failed`: a definite terminal provider/transport failure was observed;
- `cancelled_locally`: Craxii stopped waiting because cancellation won, while the provider may still finish remotely;
- `provider_outcome_unknown`: a dead runtime left no trustworthy terminal provider observation.

`requesting` deliberately includes the small window before the HTTP client transmits bytes. Recovery treats that whole window conservatively rather than claiming it knows whether a request left the process.

### `tool_executions`

```sql
tool_executions (
  tool_execution_id          TEXT PRIMARY KEY,
  execution_id               TEXT NOT NULL UNIQUE,
  work_id                    TEXT NOT NULL REFERENCES work_items,
  source_model_invocation_id TEXT NOT NULL REFERENCES model_invocations,
  runtime_instance_id        TEXT NOT NULL REFERENCES runtime_instances,
  agent_step_no              INTEGER NOT NULL,
  tool_ordinal               INTEGER NOT NULL,
  provider_tool_call_id      TEXT,
  tool_name                  TEXT NOT NULL,
  tool_version               TEXT NOT NULL,
  tool_schema_version        TEXT NOT NULL,
  arguments_json             TEXT NOT NULL,
  arguments_sha256           TEXT NOT NULL,
  workstation_id             TEXT NOT NULL REFERENCES workstations,
  workstation_generation     INTEGER NOT NULL,
  workspace_id               TEXT NOT NULL REFERENCES workspaces,
  requested_cwd              TEXT NOT NULL,
  resolved_cwd               TEXT,
  requested_privilege        TEXT NOT NULL,
  effective_privilege        TEXT,
  authority_decision_json    TEXT,
  timeout_ms                 INTEGER NOT NULL,
  output_policy_json         TEXT NOT NULL,
  state                      TEXT NOT NULL,
  requested_at               TEXT NOT NULL,
  dispatch_intent_at         TEXT,
  started_at                 TEXT,
  completed_at               TEXT,
  exit_code                  INTEGER,
  signal                     INTEGER,
  timed_out                  INTEGER,
  cancelled                  INTEGER,
  cleanup_confirmed          INTEGER,
  result_json                TEXT,
  stdout_artifact_id         TEXT REFERENCES artifacts,
  stderr_artifact_id         TEXT REFERENCES artifacts,
  stdout_observed_bytes      INTEGER,
  stdout_captured_bytes      INTEGER,
  stdout_returned_inline_bytes INTEGER,
  stdout_omitted_bytes       INTEGER,
  stderr_observed_bytes      INTEGER,
  stderr_captured_bytes      INTEGER,
  stderr_returned_inline_bytes INTEGER,
  stderr_omitted_bytes       INTEGER,
  truncated                  INTEGER NOT NULL DEFAULT 0,
  normalized_error_json      TEXT,
  UNIQUE (work_id, agent_step_no, tool_ordinal)
)
```

Tool states are:

- `requested`;
- `dispatching`;
- `completed`;
- `interrupted_before_dispatch`;
- `outcome_unknown`.

`completed` includes observed nonzero exit, timeout, cancellation, not-found, permission denial, and
validation rejection. The closed result kind inside `result_json` preserves the distinction.

The versioned `result_json` preserves the result classification. Database uniqueness additionally
enforces source-invocation/tool ordinal, optional provider-call ID within its source invocation,
and one nonterminal Tool row per Work. Adapter decode cross-checks each stream's four counts, the
global truncation fact, authority/state shape, cleanup certainty, artifact references, and safe
normalized error. Result kinds are closed and each kind rejects missing required or forbidden
exit, signal, timeout, cancellation, cleanup, and result fields.

### `artifacts`

```sql
artifacts (
  artifact_id          TEXT PRIMARY KEY,
  craxii_id            TEXT NOT NULL REFERENCES craxii_principals,
  producing_work_id    TEXT REFERENCES work_items,
  producer_kind        TEXT,
  producer_id          TEXT,
  backend              TEXT NOT NULL CHECK (backend = 'local'),
  storage_key          TEXT NOT NULL,
  sha256               TEXT NOT NULL,
  captured_byte_count  INTEGER NOT NULL,
  observed_byte_count  INTEGER,
  mime_type            TEXT NOT NULL,
  encoding             TEXT,
  logical_name         TEXT,
  retention_class      TEXT NOT NULL,
  truncated            INTEGER NOT NULL,
  compression          TEXT,
  created_at           TEXT NOT NULL
)
```

`producer_kind` is null with `producer_id`, or exactly `model_invocation` or `tool_execution` with a
canonical UUIDv7. Provenance remains per semantic Artifact ID even when several rows share the same
physical storage object. Named indexes support storage key, content, producing Work, and typed
producer lookup; no uniqueness collapses semantic occurrences.

## Transaction boundaries

### Command acceptance transaction

One message command transaction performs this exact logical order:

1. Look up `(device_id, idempotency_key)`.
2. If present, compare command type and request hash and return the stored response or conflict.
3. Lock/update the conversation row and allocate `conversation_work_ordinal` using `next_work_ordinal`.
4. Pre-generate message, work, event, and correlation IDs.
5. Insert the user message.
6. Allocate conversation stream sequence and append `message.accepted`.
7. Insert `work_items(state='queued')`.
8. Insert `work_item_inputs(relationship='trigger')` referencing the acceptance event.
9. Allocate work stream sequence and append `work.queued` caused by `message.accepted`.
10. Increment `conversations.next_work_ordinal` and projection version.
11. Construct the exact success response including the last journal offset.
12. Insert the `client_commands` row containing that response.
13. Commit.
14. Notify scheduler and live event delivery as hints.

No client can observe a committed message without its work item or vice versa.

### Work claim transaction

The claim transaction updates queued work to running and appends `work.started`. It performs no context assembly or external I/O.

### Model attempt transaction

Before outbound provider I/O:

1. Model target has already been selected.
2. Context source eligibility and manifest are finalized.
3. Persist any canonical context/request artifact bytes before the database references them.
4. Validate exact artifact-set equality and all reference/digest/count/producer/Work facts before
   inserting any artifact row or event.
5. Insert each finalized artifact's semantic metadata and append its `artifact.recorded` event.
6. Insert the context manifest and ordered source rows.
7. Insert `model_invocations(state='requesting')`.
8. Update work to `waiting_on_model` with invocation ID.
9. Append `model.invocation_started` and caused `work.waiting_on_model` in that order.
10. Commit.
11. Begin provider I/O.

The first durable valid stream observation uses one transaction to guard `requesting`, transition to
`streaming`, and append `model.invocation_streaming` while Work remains waiting on the same attempt.

After provider completion, one transaction first validates exact response artifact-set equality and
the response ID/digest/count/producer/Work facts, then inserts finalized response artifact
metadata/events, updates the invocation with caller-supplied ordered normalized output, usage, stop
reason, request IDs, and terminal state; appends the terminal invocation event; and applies only a
caller-supplied legal Work decision. Stage 8 performs no provider semantics or agent-loop decision.

When work returns to `running`, the same transaction appends `work.resumed` caused by the terminal invocation event. An exhausted provider failure transitions directly to `work.failed` instead.

### Tool execution transactions

Tool execution uses two pre-side-effect transitions:

1. **Requested transaction:** persist the complete model tool call, decoded/validated arguments or structured validation result, tool identity, workspace, concrete effective requested logical cwd, requested privilege, and `tool.execution_requested`. Set work `waiting_on_tool` and append the caused Work event.
2. **Dispatch-intent transaction:** after authority evaluation and immediately before Workstation invocation, set state `dispatching`, record effective privilege/deadline/output policy, and append `tool.execution_dispatching`. Commit.
3. **External execution:** invoke Workstation without a database transaction.
4. **Outcome transaction:** after canonical artifact bytes are finalized, validate exact stdout and
   stderr artifact-set equality and each stream's ID/digest/count/truncation/producer/Work facts;
   then insert their metadata and `artifact.recorded` events, set the caller-supplied terminal tool outcome, append
   `tool.execution_completed`, `tool.execution_interrupted_before_dispatch`, or
   `tool.execution_outcome_unknown` as exact, then apply a caller-supplied legal Work transition.

A crash after step 2 is intentionally conservative: recovery cannot know whether step 3 started, so the execution becomes `outcome_unknown` even if the process never actually spawned.

### Final-answer transaction

Stage 17 owns this transaction and `CompletionStateStore`; Stage 8 deliberately does not implement
assistant-message or final-answer persistence.

One transaction:

1. Verifies work is active, not cancel-requested, and owned by the runtime.
2. Inserts the immutable assistant message.
3. Appends `assistant.message_committed` caused by the terminal invocation event.
4. Transitions work to `completed` with outcome `answered` or `refused`.
5. Appends `work.completed`.
6. Clears current-attempt and runtime ownership fields.
7. Commits.

The client sees the committed assistant message only after this commit. A process death after commit but before delivery is repaired by cursor replay and must not run the work again.

### Cancellation transaction

Cancellation command state and its journal event, when one is required by the Stage 9 matrix,
commit with the insert-only client-command idempotency response. Stage 9 terminalizes queued Work
directly because no external action exists. Cleanup completion is a later Stage 10 transaction only
for active Work that Stage 9 placed in `cancel_requested`.

## SQLite configuration and operations

### Required pragmas

Every database connection MUST apply and verify:

```text
PRAGMA journal_mode = WAL;
PRAGMA synchronous = FULL;
PRAGMA foreign_keys = ON;
PRAGMA busy_timeout = 5000;
PRAGMA temp_store = MEMORY;
```

`synchronous=FULL` is chosen because control-state durability is more important than marginal write throughput. SQLite documents that WAL mode with FULL synchronizes the WAL on each commit, while NORMAL may lose a committed transaction after power loss. See the [SQLite synchronous pragma](https://www.sqlite.org/pragma.html#pragma_synchronous) and [WAL documentation](https://www.sqlite.org/wal.html).

Do not add performance pragmas without documenting durability effects.

### Connection model

- SQLx owns a small connection pool; four connections is the V0 default.
- All write transactions pass through one in-process `WriteCoordinator` mutex or semaphore.
- Read-only snapshot/bootstrap transactions may run concurrently.
- Write transactions use `BEGIN IMMEDIATE` or equivalent early writer acquisition.
- No transaction waits for network, process, filesystem capture, or WebSocket delivery.
- `SQLITE_BUSY` after `busy_timeout` is a storage error and metric; an internal transaction may retry only before any external effect.
- The database file, WAL, and shared-memory files remain on the same local EBS filesystem. Network filesystems are forbidden.

### Stage 5 SQLite runtime contract

The V0 implementation uses SQLx `0.9` with only `sqlite-bundled`, `runtime-tokio`, `migrate`, and `macros`; Tokio `1.53` with only `io-util`, `macros`, `net`, `process`, `rt-multi-thread`, `signal`, `sync`, and `time`; and nix `0.31` with only `fs`, `process`, and `signal`. SQLx and its bundled SQLite amalgamation own SQLite access, with no system SQLite dependency. SQLx types MUST remain inside `adapters/sqlite`; neither application nor the `StateStore` port may expose a pool, connection, transaction, query, row, SQL, database path, generic CRUD, or callback for arbitrary transaction work. Tokio process support is confined to LocalWorkstation, Tokio signal support remains owned by binary composition plus LocalWorkstation child supervision, and nix process/signal support is confined to reviewed Unix lifecycle adapters. No `tokio-util`, new futures crate, or other direct async dependency is added for Stage 13.

The database path is exactly `<state_root>/db/craxii.sqlite3` and the process-lock path is exactly `<state_root>/locks/craxii.lock`. `state_root` MUST preexist; Stage 5 may create only its `db/` and `locks/` children. Those directories are mode `0700`, and new database and lock files are mode `0600`. Production provisioning owns `state_root`, user ownership, and any `chown`; runtime MUST NOT accept a SQLite URL, URI query parameters, or a production `:memory:` mode. Missing, non-directory, symlink-leaf, unexpected-type, group/world-accessible, or reliably detectable multi-link state objects fail closed.

The mounted filesystem MUST be locally classified before use. Linux V0 allows ext2/3/4, XFS, Btrfs, tmpfs, and overlayfs. macOS V0 allows APFS and HFS/HFS+. Known remote classes including NFS, SMB/CIFS, 9p, Ceph, and identifiable network-style FUSE mounts are rejected, and unknown classes fail closed. This classification does not prove EBS backing.

In addition to the required pragmas above, every connection MUST apply and verify:

```text
PRAGMA wal_autocheckpoint = 1000;
PRAGMA locking_mode = NORMAL;
PRAGMA trusted_schema = OFF;
PRAGMA recursive_triggers = OFF;
PRAGMA secure_delete = OFF;
PRAGMA mmap_size = 0;
PRAGMA cache_size = -2000;
PRAGMA journal_size_limit = -1;
PRAGMA fullfsync = ON;
```

`fullfsync` is semantically verified where supported; Linux MUST NOT be rejected merely because the Apple-specific durability behavior is a no-op when SQLite reports the documented platform-neutral value. Any other unknown or mismatched correctness PRAGMA fails startup. The validated pool maximum is `1..=4`, default four; its minimum equals its maximum for eager validation, acquisition timeout is five seconds, and idle timeout and maximum lifetime are disabled. Replacement connections receive the same initialization and verification.

One in-process Tokio `WriteCoordinator` serializes Craxii writes. Lock order is coordinator, pooled connection, then `BEGIN IMMEDIATE`. Writes are short bounded SQLite-only transactions with explicit commit/rollback and rollback-on-drop. There is no generic retry loop, nested/savepoint public surface, or escaped transaction handle. Provider/network calls, workstation/process calls, filesystem content reads, artifact rename, client delivery, and unrelated sleeps or waits MUST NOT occur inside a SQLite transaction.

`MAX_SUPPORTED_SCHEMA_VERSION` remains zero in Stage 5. The embedded SQLx migration set contains zero Craxii migrations and may create only SQLx-owned migration metadata. There is no `user_version`, Craxii schema-version table, fake migration `0000`, or Craxii domain table. Stage 6 owns migration `0001` and the canonical schema. Preflight classifies a database as `empty`, `migrated_uninitialized`, `newer_schema`, `corrupt`, or `inconsistent`; only the first two may proceed. A positive applied migration is newer, while failed/malformed/contradictory metadata or any unexpected version-zero user object is inconsistent.

Stage 6 advances the compatibility ceiling to `1` and adds the `current` disposition. Fresh and
Stage 5 metadata-only databases apply migration `0001` and become current; version-1 databases
reopen idempotently. Stage 5's WAL, PRAGMA, filesystem, locking, integrity, transaction, and safe
error contracts remain unchanged.

Startup opens and verifies WAL on a dedicated connection, acquires a nonblocking exclusive Unix advisory lock held for the runtime lifetime, eagerly validates the pool, performs compatibility plus `quick_check` and `foreign_key_check` before migration mutation, applies the empty forward migration harness, and repeats applicable postflight checks. It never auto-repairs. The returned bootstrap guard owns the pool and lifetime lock, while successful Stage 5 startup remains `live_unready` until Stage 7 bootstrap and Stage 10 recovery are implemented.

Safe database errors expose only fixed storage, newer-schema, already-owned, corruption, inconsistency, state-conflict, or internal-invariant categories. Raw paths, SQL, bind values, SQLx/SQLite messages, content, and secrets MUST NOT enter Display, Debug, serialized normalized errors, startup diagnostics, source chains, or traces. Sanitized numeric SQLite codes are diagnostic only and MUST NOT be put in `SourceStatus::os_errno`. A passive checkpoint report is observational, not a backup; shutdown closes the pool and MUST NOT force `TRUNCATE`.

A successful `FULL`/WAL commit is treated as durable across process crash and conforming local-storage crash boundaries. This does not promise protection from filesystem or storage failure, EBS volume loss, account or Availability Zone loss, or absent backup.

### WAL checkpointing

- Configure `wal_autocheckpoint` explicitly, initially 1000 pages.
- Record WAL byte size and checkpoint latency.
- Long read transactions MUST be bounded so they do not prevent checkpoint progress.
- Bootstrap/history reads must complete and release their read transaction before opening a long-lived WebSocket.
- A maintenance path MAY issue a passive checkpoint after large writes.
- Never copy only the main `.sqlite3` file while WAL is active. SQLite treats the WAL as persistent database state.

### Migrations

- SQLx migrations are version-controlled and forward-only in V0.
- The backend may apply approved migrations automatically at startup because there is one instance.
- A pre-migration snapshot is required once real history exists.
- Migrations run before readiness and before scheduling.
- A migration failure keeps the service unready and must not partially serve commands.
- Destructive migrations are forbidden in V0. New schema versions should be additive or copy-and-verify migrations with explicit backup.
- The binary records its maximum supported schema version and refuses a newer incompatible database.

### Integrity checks

Startup MUST verify:

- `PRAGMA quick_check` returns `ok`;
- foreign-key check has no violations;
- one Craxii principal and one primary conversation exist after bootstrap;
- all active work references the current or an old runtime and has a current-attempt relationship consistent with state;
- every `work_item_input` points to an input event correlated to the same work;
- terminal work has `terminal_at` and no current attempt;
- every journal stream head matches the maximum committed sequence;
- referenced canonical artifacts exist and match expected size; hashes may be sampled at startup and fully verified in backup/restore tests.

## Idempotency

### Key scope

The Mac client generates a UUIDv7 `client_message_id` before first submission and reuses it for every retry. For a message command, this ID is also the `Idempotency-Key` value. The key is scoped by authenticated `device_id`.

The server constructs a request hash from a typed, length-prefixed tuple:

```text
protocol_version
command_type
conversation_id
client_message_id
normalized ordered content blocks
```

It does not hash arbitrary JSON serialization, so whitespace and object-key ordering do not affect identity.

### Duplicate behavior

- Same device, same key, same command type, same hash: return stored status/body and `duplicate=true` in the response projection if the protocol includes that convenience field.
- Same device, same key, different type/hash: `409 idempotency_conflict` with no domain writes.
- Different device, same key: independent scope, though `(device_id, client_message_id)` still prevents accidental same-device duplication.
- Concurrent identical requests race on the primary key; the loser reloads and returns the winner's response.

The original response body is stored so a retry after “commit succeeded, HTTP response lost” is unambiguous.

### Internal idempotency

- `execution_id` is generated before Workstation dispatch and is stable for that attempt.
- Future `RemoteWorkstation.execute(execution_id, request)` must deduplicate repeated starts.
- `LocalWorkstation` cannot prove deduplication after process death; therefore recovery never calls `execute` again for an ambiguous ID.
- Provider invocations use unique attempt IDs. Craxii does not assume provider request idempotency.

## Artifact architecture

### Artifact purpose

Artifacts keep large or binary evidence out of journal JSON, model context, and WebSocket frames while retaining provenance. V0 uses a local content-addressed store on encrypted EBS.

Examples:

- bounded stdout and stderr captures;
- normalized provider request/response snapshots when too large for rows;
- binary or large file-read evidence metadata;
- future generated files explicitly promoted as evidence.

Workspace files are not automatically artifacts. Promotion creates an immutable evidence copy and metadata record.

### Storage layout

```text
/var/lib/craxii/artifacts/
  tmp/
    <artifact-id>.partial
  sha256/
    ab/
      <full-sha256>
```

The storage key is logical and never exposed as a permanent client path.

### Commit protocol

For any artifact bytes that a database record may reference:

1. Generate the semantic `ArtifactId`.
2. Exclusively create same-filesystem `tmp/<artifact-id>.partial` with no-follow semantics.
3. Stream, drain, bound, hash, and count captured and observed bytes.
4. Flush and `sync_all` the temporary file.
5. Create or verify the digest shard and sync a newly created directory.
6. Publish without replacement to the content-addressed path.
7. Sync the containing shard after first publication.
8. Reopen the final object no-follow and fully verify type, mode, link count, size, and digest.
9. Return the finalized descriptor.
10. Insert semantic artifact metadata/event and the referencing record in one later SQLite
    transaction, then commit.

This ordering permits an orphan file after a crash but never a committed database reference to a
not-yet-published or unverified file. Startup reports temp partials and unreferenced canonical
objects without deleting either. An explicit maintenance operation may delete only an unreferenced
canonical final object at least 24 hours old after an immediate database recheck; Stage 8 schedules
no automatic deletion.

### Retention classes

- `canonical_evidence`: required to interpret a committed invocation/tool outcome; included in backups.
- `diagnostic`: useful for debugging but not required for domain reconstruction; may be pruned later.
- `regenerable`: derived snapshot or rendering; may be recreated.

V0 does not implement automatic retention deletion. The classification is required now so S3 or lifecycle policies can be added later without redefining meaning.

### Large-output policy

Capture, inline, context, and client limits are separate:

| Limit | Default | Purpose |
| --- | ---: | --- |
| stdout artifact capture | 8 MiB | Preserve bounded raw evidence |
| stderr artifact capture | 8 MiB | Preserve bounded raw evidence |
| inline model result | 64 KiB combined | Bound next context input |
| per-stream model projection | 32 KiB | Prevent one stream monopolizing result |
| WebSocket durable payload | 256 KiB | Prevent oversized frames |
| user text message | 64 KiB UTF-8 | Bound command and context growth |

When a stream exceeds capture limit, the reader continues draining and counting while discarding excess bytes. The result sets `truncated=true`, retains a head/tail projection, and reports observed versus captured size. A large output never blocks a child on a full pipe.

## Context architecture

### V0 policy

V0 uses naive full-history context because it provides a clean correctness baseline and exposes real context-growth measurements. “Full history” means all causally eligible canonical content under the active policy, not all rows in the database and not provider-side state.

V0 does not summarize, semantically retrieve, compact, or silently drop eligible history. If eligible full history cannot fit the selected target after output reserve, context assembly fails explicitly with `context_limit_exceeded` and the work item fails honestly.

### Context Assembler boundary

The assembler accepts:

```text
work_id
immutable ModelSelectionResult
assembler/context-policy/prompt versions
```

It returns:

```text
ContextPackage
  ordered canonical input items
  system/developer instructions
  tool definitions
  exact source manifest
  source and rendered byte counts
  token estimate and method
  selected target limit and output reserve
  omissions/truncations (normally empty in V0 history policy)
  package hash
```

The provider adapter translates this package to wire types. The assembler does not know Reqwest, OpenAI JSON field names, WebSocket deltas, or SQL rows. It reads through a narrow read-only `ContextSourceStore`; it does not receive the mutating State Store, selector, fallback target list, Workstation, Tool Execution Service, or provider. One assembly loads all durable eligibility facts under one bounded SQLite read transaction. The clock supplies only nonsemantic `created_at` metadata.

### Context eligibility

Let active work have conversation ordinal `N`.

Eligible history is:

1. Current system/developer instructions by explicit version.
2. Current Workstation capability summary and logical workspace identity.
3. Tool definitions registered for the selected model target.
4. Every accepted prior user message and committed eligible output belonging to conversation work ordinals less than `N`, in causal order, including accepted input whose Work later failed, was interrupted, or was cancelled.
5. The trigger input attached to work `N`.
6. Completed model output items and observed tool results already produced inside work `N`.
7. Synthetic interruption/unknown-outcome status items for earlier terminal work when relevant.
8. Provider-native continuation items only when the durable source exists; provider, provider-model, and target configuration version match the selected target; selected capability and typed native option both permit reasoning continuation; the source is the immediately preceding completed logical invocation in the same permitted Work/agent-step chain; it is not the sole semantic history; and no later interrupted or `provider_outcome_unknown` model attempt or `outcome_unknown` tool execution forms an intervening barrier. The eligibility snapshot therefore includes completed/interrupted/provider-unknown model boundaries and definite/unknown tool boundaries in durable causal order, not only completed output rows.

Ineligible history is:

- any input attached to work ordinal greater than `N`;
- ephemeral drafts;
- incomplete tool arguments;
- unobserved tool outcomes;
- secrets, environment values, auth headers, and provider API keys;
- raw tracing logs;
- current-state rows that lack a durable content/evidence source;
- provider conversation history fetched as the sole source;
- UI-only local text.

The active trigger is loaded only through its exact `work_item_inputs` `trigger` relationship and is rendered exactly once. Every history query binds the active conversation and proves Work ownership. A later accepted Work at ordinal `N+1` is excluded from Work `N` regardless of commit time or journal offset. When `N+1` eventually assembles, a result committed for prior ordinal `N` after `N+1` was accepted is visible if it is durable at assembly time.

Completed normalized output preserves original item order, including intermediate text, complete tool calls, structured data, refusal, provider-exposed reasoning summary, and compatible continuation. Reasoning summary remains a distinct provider-neutral historical item and is never converted to assistant text or used for hidden/private chain-of-thought. Partial/draft content, incomplete arguments, provider-outcome-unknown model content, and unknown correctness-bearing output are not ordinary history.

A prior Work's committed final assistant Message is authoritative conversational output and is rendered exactly once; terminal model text representing that same final answer is not also rendered. Intermediate completed output that led to tool calls remains eligible. An ordinary tool result requires one definite observed Stage 14 result paired to its unique call by Work, conversation, source model invocation, agent step, tool ordinal, provider call ID, and tool name. Orphan, duplicate, mismatched, cross-Work, and cross-conversation results fail closed.

### Prior interrupted and failed work

Earlier accepted user messages remain eligible even if their work failed or was interrupted; they are part of the relationship. Only observed outputs become ordinary output items. The assembler adds a structured synthetic status item for:

- work failed with no assistant message;
- work interrupted;
- tool outcome unknown;
- assistant draft abandoned.

The synthetic item is generated from journal facts, listed in the context manifest, and makes uncertainty explicit. It never converts unknown into failure or success.

A cancelled prior Work gets no invented cancellation status. Its accepted user input and any durable observed output remain eligible. A tool `outcome_unknown` record is never rendered as an ordinary success or failure result; its synthetic status states that execution may have occurred, its outcome is unknown, and repetition must not be assumed safe. Interrupted model attempts contribute no partial output.

### Ordering

Canonical item ordering is:

1. system instruction blocks;
2. developer instruction blocks;
3. durable Workstation capability summary;
4. logical workspace identity;
5. stable `read_file`, then `run_shell`, tool definitions;
6. prior work in conversation ordinal order;
7. within a work item, content by agent step, original model output position, and tool ordinal, with an observed paired result immediately after its call;
8. active work trigger;
9. active work model/tool trace to date.

`journal_offset` breaks ties between events in the same logical position and a stable durable ID is the final tie-breaker. Wall-clock time and map iteration never define canonical order. Provider adapters may translate roles/items but may not reorder causal tool-call/result pairs.

### Full-history cutoff

The manifest records an eligibility cutoff, not simply “all events <= offset.” It includes:

- conversation ID;
- active work ID and ordinal;
- highest prior terminal work ordinal included;
- exact work input event IDs;
- exact active-work output record IDs;
- maximum journal offset observed during the read transaction.

This prevents a later queued message committed during assembly from leaking into the active work even if its journal offset is below another active-work event committed later.

### Token budgeting

Model selection supplies:

- advertised context window;
- configured requested output tokens;
- the exact configured estimator identity/version;
- typed provider-native options.

The assembler enforces:

```text
estimated_input_tokens + reserved_output_tokens <= context_window_tokens
```

V0 sets provider truncation to disabled. It never asks the provider to drop the beginning of history automatically.

`reserved_output_tokens` is exactly the selected target's configured `requested_output_tokens`; there is no second additive Stage 16 safety-reserve field. The estimator includes conservative instruction, text/structured, tool definition, tool call/result, opaque continuation, framing, and provider-native overhead units. Its returned identity/version MUST equal the selected target configuration. Estimator failure or mismatch fails closed; no alternate estimator is chosen. Equality in the fit formula passes and one token over fails.

The complete compact canonical provider-neutral `ModelRequest` MUST also be at most 16,777,216 bytes. The assembler fully constructs and canonically serializes the request, measures its actual UTF-8 bytes, and enforces this ceiling before checking estimator identity or invoking the estimator. Exactly 16 MiB is permitted; one byte over fails explicitly as `context_limit_exceeded` without truncation or estimator work, even if that estimator would mismatch, fail, or overflow. All eligible history is mandatory: no windowing, oldest-turn pruning, retrieval, summarization, compression, clipping, tool or instruction removal, reduced output limit, or target fallback is permitted to make context fit.

### Context manifest

The context manifest is prepared before the provider call and committed only in Stage 17's atomic first-attempt transaction with its ordered source rows, invocation intent, Work transition, and journal events. Stage 16 never independently persists a successful manifest. A context-limit failure creates no fake manifest or model invocation. The manifest records, in final rendered order:

- every source event, message, invocation output, tool result, artifact, or synthetic status item;
- source hash and current record ID;
- role/item kind passed to the canonical model request;
- bytes contributed;
- transformation or inline truncation applied;
- system prompt and toolset fingerprints;
- assembler and policy versions;
- selected provider/model/config version;
- token estimator and estimate;
- context limit, output reserve, and utilization;
- rendered request hash and optional redacted request artifact.

The manifest provides provenance. It does not duplicate raw history or replace source records.

Every source row has a contiguous one-based position, exact kind, exactly one durable identity category, role/item class, the SHA-256 of canonical durable source semantics before transformation, rendered byte contribution, and explicit transform. Normal tool history uses bounded Stage 14 projections plus artifact descriptors/hashes rather than arbitrary full artifact bytes. Artifact bytes are read only through verified artifact APIs when explicit content or reconstruction verification requires them; missing, corrupt, or partial data fails closed.

The authoritative request hash is SHA-256 of the final canonical `ModelRequest`, including generated context/logical invocation IDs, selected target, instructions, ordered items, exact tools, tool choice, fixed `parallel=false`, requested output, and typed provider-native options, with no timestamp. The manifest hash covers the canonical semantic manifest envelope and ordered sources, including generated immutable IDs, but excludes its own hash field, `created_at`, assembly latency, storage paths, telemetry, and nonsemantic SQLite details. Reconstruction starts from the prepared/stored manifest and its source rows in recorded position order, reloads only those exact durable identities, verifies ownership/type/source hashes and every manifest-bound target/version/prompt/toolset/estimator identity, rebuilds with the committed IDs, and requires exact request bytes, request hash, and manifest hash. It MUST NOT query current eligibility, substitute a newer equivalent source, or reorder from current database state. A retry of an old prepared package therefore reconstructs identically after later eligible tool/model evidence or later Work acceptance changes what a fresh assembly would select.

The Stage 16 structural checker freezes the named prior-history queries to `conversation_work_ordinal < active_ordinal` and traverses assembler-reachable helpers with bounded, alias-aware provenance. The single final-request constructor MUST preserve the complete frozen canonical input and complete stable Stage 14 tool projection, MUST copy the selected target's configured requested-output limit exactly, and MUST feed that same final `ModelRequest` to canonical serialization, the byte gate, estimation, hashing, packaging, and return. The checker follows the `ToolExecutionState::OutcomeUnknown` return topology through helpers and conservative macro boundaries: that branch may return only synthetic uncertainty and never an ordinary `ToolResult`, even if a synthetic marker is separately constructed and discarded. Its compiling negative mutations cover broader/removal/offset/application-filtered causal cutoffs, direct and helper-mediated reselection, request/source/tool/output shortening, and direct/helper/alias/wrapper-mediated unknown-to-`ToolResult` conversion; positive controls retain immutable selection/input/tool inspection, exact full-value moves, bounded Stage 14 projections, definite tool results, synthetic unknown status and helper chains, complete-request byte inspection, and diagnostics-only sampling.

### Context instrumentation

For each manifest, record:

- total source count and count by source kind;
- total canonical bytes;
- rendered provider-request bytes;
- estimated input tokens;
- provider-reported input and cached tokens after completion;
- system/tool/history/current-work contribution;
- percentage of model context window;
- assembly latency;
- omissions and transformations;
- largest source contribution;
- manifest hash;
- context-limit failure count.

These measurements establish the baseline for V0.0.02. V0.0.02 should change policy only after inspecting this evidence.

## Model subsystem

### Model layering

```text
active work + required capabilities
        |
        v
InvocationIntent / cheap context statistics
        |
        v
Model Selection Policy
        |
        v
selected ModelTarget
        |
        v
Context Assembler for selected limits
        |
        v
canonical ModelRequest + ContextManifest
        |
        v
Model Gateway attempt/retry orchestration
        |
        v
Provider Adapter
        |
        v
provider API
```

Routing never occurs inside the client handler, Context Assembler, provider adapter, or Tool Registry.

### Canonical capabilities

The V0 capability model is intentionally small:

```text
ModelCapabilities
  text_input
  text_output
  custom_tool_calling
  streaming
  ordered_output_items
  structured_output
  reasoning_continuation
  context_window_tokens
  max_output_tokens
```

Each configured target snapshots:

- provider and model ID;
- enabled/disabled state;
- capability values and their source/config version;
- context and output limits;
- endpoint/account configuration reference;
- provider-native typed option set;
- token estimator identifier;
- cost class/latency class only if used for observability, not V0 routing.

Do not create a dynamic model registry service. V0 loads typed targets from startup configuration.

### Selection policy

V0 policy is deterministic:

1. Derive required capabilities from current work and toolset.
2. If an explicitly configured target is attached to the request, verify it is enabled and capable.
3. Otherwise choose the configured default target.
4. If incapable or unavailable, return `model_selection_error`; do not silently remove tools or choose an undeclared fallback.

The selection result records:

- selected target ID;
- considered target IDs;
- required capabilities;
- selection reason `explicit` or `configured_default`;
- target configuration version;
- no wall-clock timestamp in the pure in-memory Stage 15 result; Stage 17 persistence records
  observed invocation lifecycle timestamps.

No cost optimization, learned routing, fallback ladder, or task classifier is implemented.

### Canonical model request

The provider-independent request contains:

```text
ModelRequest
  logical_invocation_id
  target
  ordered_input_items[]
  instructions[]
  tool_definitions[]
  requested_output_limit
  tool_choice_policy
  parallel_tool_calls = false
  provider_native_options
  context_manifest_id
```

Input item variants include:

- user/developer/system message content;
- prior assistant content;
- tool call with canonical call ID and arguments;
- tool result paired to that call ID;
- refusal or structured content when historically relevant;
- synthetic runtime status;
- opaque provider continuation item guarded by provider identity.

### Ordered model output

The canonical response is:

```text
ModelResponse
  output_items[] in provider order
  stop_reason
  usage
  provider_request_id?
  provider_response_id?
  provider_continuation?
  provider_metadata
```

Output item variants are:

- `text` with ordered content parts;
- `tool_call` with call ID, tool name, complete raw arguments, and parsed arguments when valid;
- `structured_data`;
- `refusal`;
- `reasoning_summary` when provider exposes one for users/developers;
- `provider_opaque` for continuation data that Craxii stores but does not interpret globally;
- `unknown_provider_item` retained for diagnostics and rejected if needed for correctness.

Text and tool calls may coexist in one response. The runtime processes items in order. Text accompanying a nonterminal tool response remains invocation evidence and may be shown as an ephemeral draft, but it is not committed as the terminal assistant message.

### Runtime terminal decision

A model response is terminal only when:

- the complete provider response has been observed and normalized;
- it contains no executable tool call requiring continuation;
- its stop/incomplete reason permits completion;
- it contains user-visible text, structured output, or refusal that V0 can render;
- work cancellation has not won;
- loop limits remain valid.

A refusal is a completed work outcome with `terminal_reason=refused`, not a fabricated normal answer. Empty output without a tool call is a definite model-output error.

### Provider adapter interface

The provider port conceptually supports:

```text
capabilities(target)
estimate_tokens(target, canonical_request)
invoke_stream(target, canonical_request, cancellation)
normalize_complete(stream_state)
classify_error(provider_error)
```

The adapter owns:

- provider authentication and headers;
- endpoint and wire request construction;
- provider tool-schema translation;
- provider streaming protocol decoding;
- response item normalization while preserving order;
- provider usage and stop-reason parsing;
- request/response IDs;
- provider-native options and continuation items;
- redaction of provider errors;
- compatibility handling for unknown provider events.

The adapter does not own:

- model selection;
- context eligibility;
- retry policy;
- work state;
- tool execution;
- journal transactions;
- client protocol events.

### Provider-native escape hatches

Provider-native options are typed per adapter and included in the target configuration snapshot. They MUST NOT be a generic unvalidated JSON bag passed directly from a client or model.

Opaque continuation items record:

- provider;
- provider item type/version;
- encrypted or opaque bytes/artifact reference;
- source invocation;
- replay eligibility constraints;
- content hash.

The OpenAI adapter may reinsert an eligible OpenAI continuation item into a later OpenAI request. A different provider ignores it while retaining the historical record. Common semantics remain accessible without provider-native data.

### OpenAI Responses adapter

OpenAI is the only real provider in V0. The adapter uses Reqwest against the Responses API.

Required choices:

- Use a configured model ID; never hard-code it in agent logic.
- Set provider storage behavior explicitly to `store=false`.
- Do not send or rely on a provider `conversation` ID or `previous_response_id` for correctness.
- Send the full canonical context assembled by Craxii for each invocation.
- Define `read_file` and `run_shell` as custom function tools.
- Set `parallel_tool_calls=false` where the selected model supports the field.
- Set provider truncation to disabled; Craxii handles limits before the request.
- Set an explicit maximum output token budget.
- Preserve provider response/output item order, response ID, request ID, stop/incomplete detail, usage, and stream sequence information.
- Request and preserve encrypted reasoning continuation content when the configured reasoning model requires stateless continuation.
- Never expose provider built-in shell, computer, web, file-search, or MCP tools in V0.
- Never execute a function call until arguments and the containing response are complete.

The official Responses API represents output as an ordered array whose item count and order depend on the response and exposes streaming events for item/delta completion. It also supports explicit storage, parallel-tool-call, output-token, and truncation controls. See the [official OpenAI Responses API reference](https://developers.openai.com/api/reference/cli/resources/responses/methods/create).

OpenAI wire structs live only under `adapters/openai`. Unknown response events are counted and retained in a bounded raw diagnostic artifact. If an unknown event could change tool or terminal semantics, the invocation fails closed as `provider_protocol_unsupported`.

### OpenAI streaming

The adapter converts Server-Sent Events into internal provider-stream events:

- response created/queued/in progress;
- output item added;
- text/refusal/arguments delta;
- output item done;
- response completed/incomplete/failed;
- usage and identifiers.

The Model Gateway maintains one bounded stream accumulator. It may emit validated text deltas to Event Delivery as ephemeral drafts, but it waits for provider completion before persisting normalized output or dispatching tools.

Partial tool arguments:

- are buffered with a hard byte limit;
- are never sent to Tool Execution Service;
- are parsed only after the tool-call item is final;
- are rejected if duplicate call IDs, invalid UTF-8, invalid JSON, or schema mismatch occurs.

### Model retry policy

Each retry is a new `model_invocations` row with the same `logical_invocation_id` and context manifest.

V0 permits at most three provider attempts for one logical invocation: initial plus two retries.

Automatic retry is allowed only for a classified transient condition before any semantic draft has been exposed:

- connect failure before response bytes;
- connection reset with no output item/delta observed;
- provider 429 with bounded retry guidance;
- provider 5xx;
- provider-declared temporary unavailable;
- response-idle timeout before output.

Do not automatically retry:

- authentication or permission errors;
- invalid requests or unsupported model/tool schema;
- context-limit errors;
- safety refusal;
- malformed completed tool arguments;
- any attempt after a text/refusal/tool delta was exposed to the client;
- cancellation;
- an unknown provider protocol event affecting semantics.

Backoff uses full jitter, a 250 ms initial base, a 5-second local cap, and provider `Retry-After` guidance capped at 30 seconds. Cancellation interrupts backoff.

Provider retries may duplicate provider billing; the invocation records ambiguity. They cannot duplicate workstation side effects because no tool is dispatched until one complete successful response is persisted.

### Model limits

V0 defaults, configurable within hard bounds:

- maximum agent-loop model steps per work item: 16;
- maximum model attempts including retries per work item: 32;
- maximum tool calls per work item: 32;
- maximum ordered output items per response: 64;
- maximum raw tool-argument bytes per call: 64 KiB;
- maximum provider invocation wall time: 5 minutes;
- maximum response-idle time after stream start: 60 seconds;
- maximum work-item wall time: 30 minutes, excluding time queued.

Exceeding a loop/content limit creates a definite normalized failure; it never silently drops calls or continues infinitely.

### Stage 15 canonical contract freeze

Stage 15 is the provider-neutral contract layer, not a model runtime. It introduces no migration,
provider call persistence, Context Assembler, Model Gateway, agent loop, production WorkRunner,
real provider HTTP, public model endpoint, or client draft stream. Production remains
`live_unready`.

Configured targets are loaded once from validated startup configuration into an immutable snapshot
ordered by ascending `ModelTargetId`. The published snapshot stores targets as the private immutable
`Box<[ModelTarget]>` produced at the end of construction. Mutable vectors or validation maps are
permitted only as constructor-local builders; the published snapshot exposes no mutable receiver,
mutable target-storage reference, replacement collection, setter, lock/cell wrapper, or helper path
that can mutate or replace its catalog. Target identity reuses `ModelTargetId`, `ProviderId`,
`ProviderModelId`, and positive target-configuration version. Each target preserves enabled state,
safe endpoint/account configuration references, the complete canonical capability/limit snapshot,
requested output tokens, estimator ID/version, and validated typed native options. There is no
dynamic registration, alias, discovery, availability state, fallback ladder, or second catalog
fingerprint; the global runtime configuration fingerprint binds catalog semantics.

The deterministic selector either validates and selects the explicit target or validates and
selects the configured default. It never substitutes another target or silently removes a
requirement. Each branch calls the same exact-ID lookup/validation path with either the explicit
`ModelTargetId` or configured default `ModelTargetId`; the selected target can have no collection,
index, iteration, candidate, or fallback provenance. Missing, disabled, or incapable lookup results
propagate as their branch-specific errors. The result preserves the selected target snapshot,
`explicit|configured_default` reason, all considered target IDs in snapshot order, required
capabilities, and target configuration version.

Canonical request and response values use compact recursively key-ordered JSON and SHA-256 for
stable semantic hashing. Vector order is semantic. Stage 15 request hashes are reusable fixture
identity only; Stage 16 owns the authoritative hash after final context rendering. Requests always
freeze `parallel_tool_calls=false`. Responses preserve the seven ordered output variants defined
above, exact raw and parsed tool arguments, provider IDs, usage, optional provider continuation,
and bounded typed metadata. The hard normalized boundaries are 64 output items, 65,536 actual
UTF-8 bytes per complete raw tool-argument string, and 262,144 actual compact serialized bytes per
normalized response. Canonical response construction validates the observed-order input vector by
immutable iteration and moves that same vector unchanged into the response. Provider normalization
MUST normalize each observed semantic item through a one-input to one-output-or-error function and
append each result in observed order; it MUST NOT expose the canonical vector mutably, reorder it,
or use filtering, folding, partitioning, or subset helpers to decide whether an item survives.
Unknown items therefore become retained canonical evidence or an explicit error, never absence.
Overflow rejects the response without truncating or dropping an item.

Usage is nonnegative and signed-64-bit-compatible, with cached input no greater than input,
reasoning no greater than output, and total exactly input plus output under checked arithmetic.
The response constructor MUST enforce this terminal-consistency matrix without discarding an item:

- `completed` requires text or structured data, forbids a tool call, refusal, or unknown item, and
  permits reasoning summary or provider-opaque evidence only alongside normal answer content;
- `tool_continuation` requires at least one complete tool call, forbids refusal, structured data,
  and unknown items, and may retain accompanying text, reasoning summary, or provider-opaque
  evidence in provider order;
- `refusal` requires refusal content and forbids normal text, structured data, tool calls, reasoning
  summary, and unknown items; provider-opaque evidence may accompany it;
- `incomplete_provider_limit` may retain non-executable text, structured data, reasoning summary,
  or provider-opaque evidence, but forbids tool calls, refusal, and unknown items;
- `cancelled` has no exposed semantic output and may retain only provider-opaque evidence; and
- `provider_failure` has no usable answer, executable tool call, structured data, refusal, or
  reasoning summary. It may retain bounded provider-opaque or unknown diagnostic evidence, but an
  unknown item still fails supported-semantics validation.

Permanent response validation MUST use an independently declared stop-reason × semantic-class
matrix covering all six stop reasons and all seven canonical semantic classes (42 single-semantic
cells), plus explicit meaningful mixed-item cases. Expected validity MUST NOT be derived from the
response constructor or a production semantic classifier, allowed-mask helper, or compatibility
function.

Unknown provider items and events retain bounded hashed evidence and fail closed when semantics are
required. Reasoning output is limited to provider-exposed summary; no hidden/private chain-of-
thought type exists.

The internal stream contract records started identity, text/reasoning/refusal deltas, tool start and
bounded argument deltas, tool completion, structured data, usage, explicit `usage_unavailable`,
completion, provider error, and unknown event evidence. One small state machine preserves item
ordinal/call identity and enforces one initial started event, no event before it, unique call IDs,
exact start/delta/completion identity, the aggregate argument bound, no open call at successful
completion, exactly one usage result immediately before exactly one terminal event, and no event
after usage or terminality. A successful normalized response MUST use exactly one `usage` event,
whose value equals the terminal response usage. A provider-error terminal MUST use exactly one of
`usage` or `usage_unavailable`; zero usage is never synthesized. The terminal response target MUST
equal the started target. No validator may reorder events to repair an invalid stream.

Provider-error terminal mapping preserves certainty: authentication, authorization, invalid
request, unknown model, context, safety/refusal, and internal failures are definite provider
failure; rate limit, temporary unavailable, and transport-before-response are definite transient
failure; cancellation is cancellation; transport-after-possible-processing and provider-outcome-
unknown are outcome ambiguity; timeout-before-output and timeout-after-output remain distinct; and
malformed, oversized, or unsupported responses are protocol failure. A timeout-before-output or
transient-before-output terminal after semantic output, and a timeout-after-output terminal before
semantic output, are contradictory and MUST fail validation. No such terminal is `completed`. The
contract is not an HTTP/SSE contract.

The object-safe provider and token-estimator ports receive already-selected provider-neutral
values. Retry classification is pure and permits at most initial plus two retries. Only definite
transient failure before semantic output is eligible. Authentication/authorization/request/model/
schema/context/refusal/malformed/unsupported/cancellation/ambiguous/deadline-exhausted and every
post-output failure are nonretryable. Backoff is pure full jitter with 250 ms base, 5-second local
cap, and Retry-After capped at 30 seconds; it never sleeps in Stage 15 tests.

One attempt freezes the minimum of absolute Work, shutdown, provider-invocation, and retry-budget
deadlines. The default provider limit is five minutes and stream idle ceiling is 60 seconds. A
known absolute deadline is passed unchanged rather than reconstructed from a fresh relative
timeout. Cancellation is provider-neutral and awaitable. `ScriptedProvider` retains the injected
monotonic clock, absolute deadline, idle timeout, last valid activity, cancellation token, and the
same semantic-output predicate used by retry classification. Before each event and after each
release barrier it checks cancellation, then overall and idle expiry. Cancellation wins a same-
observation race; `now >= threshold` is expired. Timeout before semantic output is the definite
pre-output classification, while timeout after text, tool, refusal, structured data, reasoning
summary, or unknown semantic evidence is the nonretryable post-output classification.

The deterministic `ScriptedProvider` is Stage 15's only concrete provider adapter and is never
composed as a live provider. It matches target/request hash or fixture key/prior tool result/
invocation ordinal/attempt, emits ordered provider-neutral events or classified errors, and uses
injected release barriers rather than sleeps. It records redacted IDs, hashes, counts, ordering,
cancellation, and terminal classification while retaining exact requests only through explicit
test-safe capture access. A program with any step after its first emitted terminal event or terminal
failure is rejected before the first event; capture distinguishes completion, provider failure,
cancellation, outcome unknown, timeout, script mismatch, and invalid program. Its canonical
scripted estimator uses immutable estimator identity plus target identity and exact normalized unit
sequence as the lookup key. Repeated equivalent input returns the same nonnegative estimate without
consuming state; each programmed estimate MUST be at least the checked sum of its byte units, and a
missing or duplicate key fails deterministically.

The Stage 15 provider contract tests MUST be reusable through `ModelProvider` plus a narrow fixture
factory, without reading a concrete provider's fields, stream type, program queue, or captures. The
suite covers identity/correlation, ordered text and multiple complete tool lifecycles, exactly one
usage result and terminal, refusal, structured data, reasoning summary, opaque continuation,
unknown-item fail-closed behavior, limits, cancellation, overall and idle deadlines, certainty
mapping, semantic-output observation, and terminal response validation. Scripted-provider-specific
tests remain additional coverage.

The Stage 15 structural checker MUST resolve bounded simple local type aliases and traverse
reachable helpers with a visited set and a finite conservative depth bound. It MUST reject runtime
target-catalog mutation across aliases and helpers, selector paths whose explicit/default target
provenance changes, non-append provider-output mutations or split/recombine transformations, and
normalization paths where an unknown item can disappear without normalized output or explicit
error. Permanent Stage 15 semantic checker mutations MUST first pass a deterministic Rust
compilation gate and only then prove checker rejection; a noncompiling mutation is a failed probe.
Its retained compilation-gated controls MUST also prove that constructor-local mutable target
builders, target-ID projection and diagnostic iteration, deterministic target-inventory sorting,
canonical JSON key sorting, and unrelated fixture filtering remain allowed.
`ScriptedProvider` does not persist, journal, execute tools, access a Workstation, use HTTP, or know
provider wire structs.

## Explicit agent loop

### Agent-loop ownership

One agent-loop task owns one claimed work item. The loop is a normal explicit Rust control flow. No callback, provider SDK, workflow framework, or Tool Registry entry may recursively invoke the model.

The loop receives:

- `work_id` and current `state_version`;
- owning `runtime_instance_id`;
- cancellation token linked to durable cancellation state;
- State Store, Context Assembler, Model Gateway, Tool Execution Service, Event Delivery, and clock dependencies;
- immutable runtime limits.

### Step-by-step algorithm

The normative loop is:

1. Reload work and verify runtime ownership and nonterminal state.
2. Check cancellation and total work deadline.
3. Derive required model capabilities from the registered toolset and current content.
4. Gather cheap eligibility/context statistics sufficient for selection without rendering provider wire input.
5. Select a `ModelTarget`.
6. Assemble canonical context and exact manifest for that target.
7. Fail explicitly if full eligible context does not fit.
8. Persist context manifest and model invocation intent; set work `waiting_on_model`.
9. Call the provider through Model Gateway.
10. Stream safe text/refusal deltas as ephemeral drafts.
11. Persist the complete normalized ordered response and observed usage; return work to `running` unless cancellation/failure wins.
12. Inspect ordered output items.
13. If one or more tool calls exist, process them sequentially in provider order through Tool Execution Service.
14. For each tool result, persist its full terminal evidence before making it eligible for context.
15. After all requested calls have observed results, increment `agent_step_no` and return to step 1.
16. If no tool call exists and terminal output is valid, atomically commit assistant message and work completion.
17. If the response is incomplete, invalid, over limits, cancelled, or definitively failed, transition according to the normalized condition.

The loop never constructs an assistant message from an incomplete stream buffer.

### Multiple tool calls

V0 asks providers not to generate parallel calls but remains correct if several appear.

- Preserve provider order.
- Validate every call before executing that call.
- Execute calls sequentially.
- Persist each result independently.
- If cancellation arrives, do not start later calls.
- If a call yields an observed ordinary error, include it and continue to later calls unless policy says the later call depends on unsafe unknown state.
- If a call becomes `outcome_unknown`, stop the work immediately and mark it interrupted.

V0 does not attempt transactional rollback across tool calls.

### Text mixed with tools

If a response contains explanatory text before a tool call:

- retain it in ordered invocation output;
- it may appear as an ephemeral draft;
- include it in the next same-work context if the provider contract requires it;
- do not commit it as the final assistant message;
- abandon the draft if the invocation/work later fails.

Only a no-pending-tool terminal response produces a committed assistant message.

### Loop failure behavior

| Condition | Tool result sent back to model | Work terminal action |
| --- | ---: | --- |
| `read_file` not found | Yes | Continue loop |
| shell exits nonzero | Yes | Continue loop |
| shell times out but cleanup confirmed | Yes | Continue loop unless limit/cancel reached |
| tool arguments invalid | Yes, structured validation error | Continue loop |
| unknown tool name | Yes, structured error | Continue loop; repeated abuse may hit loop limit |
| provider transient failure exhausted | No | `failed` |
| context too large under full-history policy | No | `failed(context_limit_exceeded)` |
| internal storage inconsistency | No | Fatal/unready; work not guessed |
| tool outcome unknown | No invented result | `interrupted` |
| user cancellation with cleanup confirmed | No | `cancelled` |
| agent loop limit exceeded | No | `failed(agent_loop_limit)` |

### No hidden retries

The agent may choose to call a tool again after seeing an observed failure. That is a new explicit model tool call with a new execution ID, not an infrastructure retry. The Tool Execution Service never repeats a call on its own in V0.

## Tool architecture

### Tool layering

```text
Agent Runtime
    |
    v
Tool Execution Service
    |-- state validation
    |-- intent persistence
    |-- authority-decision seam
    |-- registry lookup and argument validation
    |-- outcome persistence
    |
    v
Tool Registry
    |
    v
typed Tool Handler
    |
    v
Workstation port
    |-- LocalWorkstation in V0
    +-- RemoteWorkstation later
```

Model-facing tools remain above the Workstation. `run_shell` is a tool; `execute` is a machine primitive.

### Tool definition

Each registered tool definition includes:

- stable name;
- semantic version;
- schema version;
- concise model-facing description;
- input JSON Schema;
- typed Serde input decoder;
- canonical result type;
- default timeout and hard maximum;
- output policy;
- required Workstation capabilities;
- whether a side effect is possible;
- supported privilege modes;
- handler implementation.

The registry is immutable after startup. Its ordered definitions are fingerprinted for each context manifest.

The JSON Schema supplied to a provider and the typed decoder MUST be generated from one source or
proven equivalent by executable boundary tables. The equivalence test independently evaluates the
actual emitted JSON Schema AST; it does not call typed-decoder grammar helpers, fails on unknown
emitted keywords, covers every configured and hard boundary, and proves sensitivity with in-memory
schema mutations. Every schema-accepted value MUST pass the typed decoder in the same semantic
context. Byte-bounded model strings use an exact ASCII schema subset
so JSON Schema character counts cannot admit a value that violates a UTF-8 byte ceiling. Path-kind
branches encode their deterministic relative/absolute grammar in schema; filesystem existence,
symlink, and workspace-containment facts remain semantic validation because schema cannot observe
them. Provider schema acceptance is not runtime validation. Input structs deny unknown fields
unless a field is intentionally forward-compatible. Model-visible defaults and limits come from
the validated typed runtime tool configuration, and the registry fingerprint includes those
effective semantics.

The completed encoded-argument envelope accepted by the Stage 14 decoder is at most 524,288 bytes.
This transport ceiling is intentionally larger than the 65,536-byte command field because JSON
escaping can expand a schema-valid ASCII value; it MUST NOT silently narrow an advertised field
limit. The typed decoder still applies field limits and duplicate-key rejection before any durable
requested transition.

### Tool Registry

The registry owns:

- startup registration and duplicate detection;
- lookup by name/version;
- stable definition ordering;
- schema/toolset fingerprinting;
- capability reporting;
- typed handler resolution.

It does not own:

- work state;
- permission decisions;
- persistence;
- timeout orchestration;
- retries;
- model invocation;
- client progress events.

Unknown tool names produce a structured `unknown_tool` result. They never invoke a shell fallback.

### Tool Execution Service

For each complete model tool call, the service:

1. Verifies the source model invocation is completed and belongs to the active work.
2. Verifies the provider call ID is unique within that invocation.
3. Resolves the exact registered tool and schema version.
4. Parses and validates arguments into a typed input.
5. Creates a stable `tool_execution_id` and `execution_id`.
6. Injects work, workspace, workstation, deadline, output policy, and authority context; these are never trusted from hidden model fields.
7. Freezes the one absolute monotonic deadline from the invocation start, requested/default tool
   timeout, upstream Work deadline, and an already-active shutdown deadline.
8. Commits `tool.execution_requested` and work `waiting_on_tool`.
9. Checks cancellation again, obtains current Workstation capabilities, and evaluates the V0
   authority policy to resolve requested to effective privilege.
10. Uses the separate non-side-effecting Workstation preparation port to bind the exact
    workstation/generation/workspace, validate capability and privilege feasibility, and resolve
    adapter-observed cwd evidence without spawning, executing a command, reading tool content, or
    creating machine state.
11. Commits `tool.execution_dispatching` with that resolved evidence before machine access.
12. Invokes the typed handler.
13. The handler uses only the injected Workstation for machine access.
14. Finalizes evidence artifacts.
15. Commits the canonical result, cleanup evidence, and tool terminal event.
16. Returns the bounded canonical model result to the agent loop.

The service, not the handler or registry, owns journal ordering.

The effective timeout is frozen once as one absolute process-local monotonic deadline before
requested persistence or capability acquisition and carried unchanged through authority,
preparation, dispatch, handler, and Workstation. It MUST NOT be reconstructed from a later `now`;
expiration during any pre-handoff phase causes no machine operation. Only bounded duration
evidence, never the monotonic instant, is durable.

Shell handoff has an explicit pre-handoff state. If cancellation wins before the one handler future
is first polled, that future is dropped unpolled, no Workstation action/cancellation call occurs,
and the result is definitely cancelled. Once handoff is claimed, cancellation targets the exact
`ExecutionId` while the service awaits that same lifecycle; no future is detached or later polled
into starting after cancellation.

Large `read_file` evidence artifacts are generic tool artifacts referenced by canonical result
metadata, never stdout/stderr artifacts. Startup consistency validates their exact tool/work
producer, role, hash, size, MIME/encoding, and reference cardinality. Shell result projection is
bounded by actual canonical serialized bytes: UTF-8-safe inline fields shrink deterministically to
remain within the V3 262,144-byte result ceiling while full stream artifacts, hashes, counts,
omitted-byte truth, and definite process classification remain intact.

The five Stage 14 crash hooks are active only with test failpoints: after requested commit, after
dispatch-intent commit, after process spawn, after terminal observation and cleanup classification
but before outcome commit, and after artifact rename before DB commit. Subprocess tests reach each
hook only through its production callsite and use the real Tool Execution Service lifecycle,
durable SQLite, production-compatible adapters, and markers to prove conservative recovery,
zero-or-one side effect, no redispatch, coherent Work/journal projection, no duplicate outcome, and
correct orphan handling.

### Authority decision seam

V0 has a deliberately simple authority evaluator:

```text
input:
  craxii_id
  work_id
  workspace_id
  tool identity
  normalized arguments summary
  requested privilege
  explicit user constraints known to the work

output:
  decision = allow | deny
  effective privilege
  policy version = v0-development-workstation
  reason code
```

The V0 policy allows registered local tools on the development workstation. Administrative mode is
allowed only for a tool that supports it, an explicit administrative request, a permitting policy
decision, and a current Workstation capability snapshot that reports administrative privilege.
Malformed, over-limit, cancelled, mis-scoped, stale-generation, unsupported-capability, and
authority-widening requests are denied. The complete canonical evaluator evidence—not merely its
compact decision snapshot—is recorded with every dispatched execution. It binds policy, decision,
reason, requested/effective privilege, capability and bounded-request facts, workstation generation,
workspace, Work/runtime identity, and tool identity without raw arguments, command, content, output,
secrets, or process-local handles.

This seam is not a mature Authority Service and does not machine-enforce natural-language prohibitions. Later it can call an external policy/credential plane without changing Tool Handler or model schemas.

### Tool result envelope

Every result delivered to the model contains:

```text
tool_execution_id
tool name/version
status = completed | error | outcome_unknown
result_kind
human-readable bounded summary
structured fields specific to tool
duration
truncation metadata
artifact IDs when relevant
privilege used
cleanup status when a process was involved
normalized error code/details when relevant
```

The model never receives local artifact storage paths, provider secrets, backend DB paths, raw stack traces, or arbitrary environment dumps.

## Workstation boundary

### Workstation purpose

The Workstation port makes “the computer Craxii operates” an explicit dependency. It prevents direct filesystem/process APIs from spreading through the agent runtime and provides the extraction seam for a later remote, replaceable workstation.

It is not a sandbox framework, tool registry, or authority service.

### Initial interface

The V0 interface exposes only these low-level, remote-capable operations:

```text
capabilities() -> WorkstationCapabilities

read_file(FileReadRequest) -> FileReadResult

execute(ExecutionRequest) -> ExecutionResult

inspect_execution(execution_id) -> ExecutionInspection

cancel_execution(execution_id) -> CancellationResult
```

Methods are asynchronous and cancellation-aware. Requests carry stable IDs generated above the adapter.

Do not initially add methods named:

- `install_package`;
- `clone_repo`;
- `run_tests`;
- `run_docker`;
- `manage_service`;
- `configure_toolchain`;
- `deploy`.

Those are higher-level tool/workflow semantics composed from generic execution until a repeated need justifies a typed primitive.

### Non-side-effecting preparation seam

The frozen five-method `Workstation` interface is not expanded for Stage 14. A separate
provider-neutral `WorkstationPreparation` port exposes one cwd/execution preparation operation used
only between the committed requested transaction and the dispatch-intent transaction. The
LocalWorkstation implementation:

- validates the exact Workstation ID and generation;
- validates the exact workspace binding;
- checks the required filesystem/foreground-execution capability and effective privilege
  feasibility against its current capability snapshot;
- resolves the requested logical cwd through the same adapter-owned path rules used by execution;
- verifies that the resolved target is an existing directory; and
- opens the resolved directory non-mutatingly, observes its directory type and stable filesystem
  object identity, and returns `PreparedCwdEvidence`: adapter-observed `ResolvedPathEvidence` plus
  device/inode identity bound to the workstation, generation, workspace, and requested logical cwd.

Preparation MUST NOT spawn or execute a command, read requested tool content, create a cgroup or
artifact, mutate the filesystem, persist or journal, or select broader authority. It is not a sixth
Workstation method and carries no provider, model, context, or agent-loop type. The actual
`Workstation.execute` call revalidates the request and uses its own opened-directory handle, so the
prepared evidence is durable dispatch evidence rather than a transferable OS handle or an
authorization token. The dispatch transaction durably records the canonical path, expected
directory type, device, and inode in the existing versioned dispatch evidence JSON; no raw
descriptor is serialized. Execution opens the committed resolved directory without following a
final symlink, obtains metadata from that exact descriptor, and requires both the logical binding
and the stable directory-object identity to match before spawn. The child cwd uses that same opened
and validated descriptor, with no later pathname resolution. Symlink drift, same-path replacement,
disappearance, generation mismatch, workspace mismatch, or object-type mismatch is a definite
pre-start failure. The opened-directory handle stays process-local and never enters a durable or
public protocol type. A preparation failure is definitively before dispatch and causes no machine
action.

### Workstation capabilities

Capabilities are a versioned snapshot:

```text
workstation_id and generation
kind = local
operating system/release
CPU architecture
default shell executable
filesystem read support
foreground execution support
cancellation support
inspection support
user/admin privilege modes
process-group and cgroup cleanup support
maximum timeout/output values
default workspace IDs and logical roots
```

Capabilities are evidence and selection input. They do not grant authority.

### File request

```text
FileReadRequest
  operation_id
  workstation_id + generation
  workspace_id
  path reference
  maximum bytes
  encoding policy
  deadline
```

A path reference is one of:

- workspace-relative path, resolved from the injected workspace root;
- absolute machine path, allowed in this broad-authority V0 and recorded explicitly.

The adapter returns both requested/logical and resolved paths. A future remote adapter may use different physical paths while preserving logical identity.

### Execution request

```text
ExecutionRequest
  execution_id
  workstation_id + generation
  workspace_id
  command form
  requested/resolved cwd
  privilege mode
  sanitized environment specification
  stdin policy
  timeout/deadline
  stdout/stderr capture policy
  cancellation handle
  resource/cleanup policy
```

The Workstation receives effective privilege, not the model's unchecked request.

### Execution result

```text
ExecutionResult
  execution_id
  start observed?
  resolved cwd
  effective privilege
  result kind
  exit code or signal
  timeout/cancel flags
  monotonic duration
  stdout/stderr capture streams or finalized descriptors
  observed/captured byte counts
  truncation
  process-tree cleanup result
  normalized workstation error
```

Local PIDs may appear in tracing but not as durable execution identity.

### Workstation error taxonomy

The port returns normalized machine errors:

- `workstation_unavailable`;
- `generation_mismatch`;
- `workspace_not_found`;
- `invalid_path`;
- `not_found`;
- `permission_denied`;
- `binary_content`;
- `file_too_large`;
- `changed_during_read`;
- `unsupported_capability`;
- `spawn_failed`;
- `timeout`;
- `cancelled`;
- `signal_terminated`;
- `output_truncated` as result metadata, not fatal error;
- `inspection_not_found`;
- `cleanup_failed`;
- `io_error`;
- `internal_workstation_error`.

Linux `errno`, paths, and process details remain in redacted diagnostic detail and do not leak across every application layer.

## LocalWorkstation

### LocalWorkstation ownership

`LocalWorkstation` is the only production V0 adapter allowed to call ordinary local filesystem and process APIs for model-requested machine actions.

Direct `std::fs`, `tokio::fs`, `std::process`, or `tokio::process::Command` usage elsewhere is allowed only for backend-owned state/config/artifact infrastructure, never for a model-facing machine operation.

Static analysis or code review SHOULD enforce this boundary by module visibility.

### Execution identity and handles

- The Tool Execution Service creates `execution_id` before dispatch.
- LocalWorkstation maintains an in-memory map from execution ID to live process group/cgroup handle.
- The map is ephemeral and scoped to one runtime instance.
- `inspect_execution` returns observed live/terminal status only for handles known to the current runtime.
- After process restart, an absent handle cannot prove nonexecution. Recovery relies on the durable dispatch state and uses `outcome_unknown`.
- The live registry is keyed only by `ExecutionId`, rejects a duplicate live ID before spawn, and
  owns one observed supervisor plus its drain tasks until terminal cleanup. Dropping the caller
  awaiting `execute` does not transfer or abandon that ownership. Terminal entries exist only for
  coherent result handoff and are then removed; this is not durable terminal history.

### Foreground-only contract

`run_shell` is a bounded foreground execution primitive.

- Arbitrary shell backgrounding, `nohup`, double-fork daemons, and interactive terminal sessions are unsupported.
- Descendants that remain in the execution process group/cgroup are terminated at command completion/cancellation.
- An explicit `systemctl start/stop` command is allowed to create or manage an operating-system service outside the foreground execution lifecycle. That is a recorded non-idempotent machine side effect, but V0 does not track the service as a durable Workstation execution.
- A future `start_process`/`inspect_process` tool will own durable local development servers; it is deferred.

### Process containment and cleanup

Every foreground execution MUST have:

- a new Unix process group/session;
- `kill_on_drop` as defense in depth, not the primary cleanup mechanism;
- membership in a per-execution cgroup v2 subtree or equivalent systemd-created scope;
- non-reaping direct-child terminal observation followed by descendant cleanup while the waitable
  session leader still pins its PID/process-group identity, then direct-child reaping;
- concurrent stdout/stderr drains;
- cgroup emptiness verification before terminal cleanup is reported.

No Unix cleanup path may deliver a process-group signal after the waitable leader identity has been
released. Natural exit, cancellation, timeout, and shutdown MUST all complete their final
process-group signal and quiescence proof before reaping the leader. Linux cgroup identity remains
the definitive containment authority; numeric process-group signalling is only early escalation
there, and `cgroup.kill` plus cgroup emptiness remain valid independently of PID/PGID reuse.

The systemd backend unit uses `KillMode=control-group` so backend death kills ordinary descendant processes. Per-execution cgroups allow cancellation to target one command tree.

On Linux the validated shell configuration supplies an optional absolute
`delegated_cgroup_root`, which MUST be a child of `/sys/fs/cgroup`. Linux foreground execution is
advertised only when LocalWorkstation can create, inspect, and remove an empty probe cgroup beneath
that exact root. Every execution creates one child named from its non-PID `ExecutionId`; the narrow
child `pre_exec` performs `fchdir`, `setsid`, and a write of `0` to that already-created
`cgroup.procs` before Bash code can run. On macOS, local verification may advertise only the
process-session/group approximation: cgroup and administrative capability remain false.

Tokio documents that a child continues by default when its handle is dropped; `kill_on_drop` affects the direct child and is not a substitute for descendant cleanup. See [`tokio::process::Child`](https://docs.rs/tokio/latest/tokio/process/struct.Child.html).

An intentionally hostile root process can escape local cleanup. V0's guarantee is against ordinary compilers, test runners, shells, and accidental daemonization, not adversarial root code.

### Privilege semantics

There are two explicit domain values:

- `user`: run as Unix user `craxii` without elevation;
- `administrative`: run through noninteractive root escalation.

The backend process itself runs as `craxii`, not root. Administrative execution uses an explicit `sudo -n` path or equivalent controlled launcher. The final command environment is constructed from scratch after elevation; the backend environment is never preserved wholesale.

The sudo policy permits Craxii to administer this development workstation autonomously. It is intentionally broad. Every elevated call records requested and effective privilege.

The reviewed V0 administrative launcher is exactly absolute `/usr/bin/sudo -n`, followed by
absolute `/usr/bin/env -i`, the fixed root allowlist, and the same direct Bash argument sequence.
Administrative capability remains false unless configuration enables it and a noninteractive,
side-effect-free runtime probe observes root identity, the clean root values, and working delegated
cgroup ownership. `sudo -E`, a password prompt, a root backend, setuid backend, and an unreviewed
helper daemon are forbidden.

The model-facing `run_shell` input may request administrative mode. That request is not self-authorizing: Tool Execution Service resolves it through the V0 authority evaluator and records the decision.

### Why one Unix user

One user is chosen because:

- backend and workstation are already one accepted V0 trust domain;
- a second user plus IPC/executor daemon would add process, filesystem, and debugging complexity;
- root or Docker access could cross that separation anyway;
- the lasting extraction seam is `Workstation`, not local UID topology.

State and workspace paths remain separated operationally to reduce accidental corruption. The architecture does not claim that the model-controlled shell cannot discover or alter backend state.

## `read_file` tool

### `read_file` model-facing input

```text
path                  required UTF-8 string
path_kind             workspace_relative | absolute, default workspace_relative
max_bytes             optional, default 1 MiB, hard maximum 8 MiB
```

Workspace identity is injected, not model-specified.

### Semantics

1. Reject NUL, empty, or overlong path input.
2. Resolve a relative path against the work item's workspace root.
3. Resolve symlinks/canonical path for an existing target and record both requested and resolved paths.
4. Open/read under the effective `user` privilege in V0.
5. Obtain file type, size, and modification metadata.
6. Reject directories and unsupported special files.
7. Read no more than the requested/hard limit.
8. Validate UTF-8.
9. Compute SHA-256 over returned bytes.
10. Return structured content and metadata.

Binary/non-UTF-8 content produces `binary_content` with size/hash metadata where safe; it is not lossy-decoded and presented as text. A file larger than the limit returns `file_too_large` unless the chosen behavior explicitly returns a marked prefix; V0 default is an error to avoid pretending the file was fully read.

### Result

```text
requested path
resolved path
file type
byte length
modified timestamp?
UTF-8 encoding
SHA-256
content when successful
truncated = false in V0 default
structured error when unsuccessful
```

No result fabricates empty content for a failure.

## `run_shell` tool

### Why a shell string

Real engineering uses pipes, redirects, environment assignment, globbing, and compound commands. V0 therefore exposes a noninteractive Bash command string rather than pretending every operation is a direct executable/argument array.

The shell string is expected to be arbitrary code on Craxii's own development workstation. Quoting protects the launcher boundary; it is not an attempt to sanitize the command itself.

### `run_shell` model-facing input

```text
command               required UTF-8 string, maximum 64 KiB
cwd                   optional path reference, default workspace root
privilege             user | administrative, default user
timeout_seconds       optional, default 120, hard maximum 900
```

No generic secret or credential fields exist.

### Shell invocation

Use a non-login, noninteractive Bash invocation with profiles disabled:

```text
/bin/bash --noprofile --norc -o pipefail -c <command>
```

Each call starts a fresh shell.

- `cd` affects only that invocation.
- Shell variables and current directory do not persist to the next call.
- `stdin` is closed or `/dev/null`.
- stdout and stderr are separate pipes.
- The result always reports the resolved starting cwd.
- The backend MUST NOT interpolate the command into another shell layer while constructing the process.

### Working-directory semantics

- A missing `cwd` resolves to the injected workspace root.
- A relative `cwd` resolves under that root.
- An absolute `cwd` is accepted in the broad-authority V0 and recorded.
- The directory must exist and be a directory before dispatch.
- Path traversal is normalized for evidence and correctness, not advertised as a sandbox.
- The Workstation, not the agent loop, performs OS path resolution.

### Environment construction

The child environment is built from an allowlist, not inherited.

Default user-mode environment:

```text
HOME=/home/craxii
USER=craxii
LOGNAME=craxii
SHELL=/bin/bash
LANG=C.UTF-8
PATH=/home/craxii/.local/bin:/home/craxii/.cargo/bin:/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin
CRAXII_WORK_ID=<non-secret ID>
CRAXII_WORKSPACE_ID=<non-secret ID>
```

Admin mode sets root identity values deliberately and invokes a clean environment after `sudo`. It never uses `sudo -E` or preserves the backend environment wholesale.

Explicitly absent unless a future authority adapter injects them for one operation:

- provider API keys;
- client bearer tokens;
- database path/config secrets;
- AWS/GitHub/customer credentials;
- HTTP authorization headers;
- Rust tracing filters containing sensitive values;
- open file descriptors other than standard streams and launcher control handles.

Project commands may set ordinary variables inside the shell string. That does not grant access to secrets absent from the machine environment.

`ExecutionRequest` contains no generic environment vector or override. It carries the canonical
nonsecret `WorkId` and `WorkspaceId`; LocalWorkstation alone derives the two Craxii variables. The
launcher calls `env_clear`, sets only the eight listed values, marks every observed unrelated parent
descriptor close-on-exec, and uses `/dev/null` for stdin. Bash may maintain its own documented shell
variables after exec; that does not authorize additional parent environment inheritance.

### Output capture

- Drain stdout and stderr concurrently from process start.
- Preserve byte streams independently; do not merge order and claim it is exact.
- Capture up to 8 MiB per stream into artifacts.
- Continue draining after the cap to prevent child deadlock.
- Count observed bytes with a saturating 64-bit counter.
- Decode model projection as UTF-8; replace invalid sequences only in a clearly marked display projection while retaining binary artifact metadata.
- Produce a 32 KiB per-stream model projection using 24 KiB head plus 8 KiB tail when truncated.
- Report truncation and omitted-byte counts.
- Never write raw output into tracing by default.

### Result semantics

```text
command hash and optional redacted summary
requested/resolved cwd
effective privilege
result_kind = exited | signaled | timed_out | cancelled | spawn_failed | cleanup_failed
exit_code?
terminating_signal?
duration_ms
stdout/stderr inline projections
artifact IDs
observed/captured byte counts
truncation flags
process-tree cleanup status
```

Exit code zero is normally success. A nonzero exit is still `state=completed`, `result_kind=exited`, and is returned to the model. Spawn failure is distinct from exit 127. Signal termination is distinct from timeout.

### Timeout and termination

- Default timeout: 120 seconds.
- Hard model-requestable maximum: 900 seconds.
- On deadline, mark timeout requested, send SIGTERM to the process tree, wait 5 seconds, then SIGKILL.
- Reap and verify cgroup cleanup.
- Return `timed_out=true` only after observation/cleanup.
- If cleanup is unconfirmed, use `outcome_unknown` and interrupt work.

The execution runtime budget and the absolute Workstation operation deadline are distinct monotonic
bounds. Cancellation, timeout, natural exit, and shutdown race through one first-cause latch.
Shutdown uses the already-latched Stage 10 absolute deadline; LocalWorkstation closes only new
execute admission, cancels live entries, joins supervisors/drains, and does not create a secondary
deadline or a second shutdown authority.

Execute admission and insertion into the live `ExecutionId` registry share one short-held authority
lock. Before insertion the execution is unowned and shutdown may reject it; successful insertion is
the ownership boundary, after which shutdown must cancel and join that owned lifecycle rather than
reclassifying it as unavailable or spawn-failed. A per-entry first-cause/spawn claim decides whether
shutdown cancellation prevents OS spawn with `start_observed=false` or whether an already claimed
spawn proceeds under ownership into the ordinary TERM/KILL/reap/drain path. No registry lock is held
across a spawn, OS call, or await.

Direct-child terminal observation uses one non-reaping
`waitid(WEXITED | WNOHANG | WNOWAIT)` attempt per cooperative supervisor poll. `EINTR` is an
interrupted nonterminal observation, not exit, liveness proof, `ECHILD`, or cleanup failure; control
returns to the async cancellation and deadline machinery before another attempt. A zero observed PID
is pending, true terminal states retain the waitable leader until descendant cleanup releases the
stable group identity, and only then may the child be reaped.

### Stage 13 target verification status

Stage 13 production code, macOS/Unix-process-group tests, Linux cgroup/admin code, Linux-gated tests,
and crash-marker harnesses are implemented locally. Ubuntu 24.04 x86-64 execution under the real
non-root `craxii` systemd unit with `Delegate=yes`, `KillMode=control-group`, reviewed `sudo -n`,
Docker, service restart, and reboot leak scans is a target-environment acceptance gate. Deferring
that gate does not permit optimistic Linux cgroup/admin capability flags and does not block Stage 14
implementation after the local Stage 13 gate passes.

### Administrative operations

The same tool can install packages, install toolchains, run Docker, edit system configuration, and manage services by requesting administrative privilege. These are ordinary workstation capabilities and need no user approval in V0.

They remain non-idempotent. A crash after dispatch creates unknown outcome and no automatic retry.

## Future RemoteWorkstation seam

### Required behavioral equivalence

A future RemoteWorkstation may use RPC, SSH-like transport, or a dedicated executor protocol. The agent runtime must not care. It must preserve:

- caller-generated stable `execution_id`;
- workstation ID and generation checks;
- logical workspace/path semantics;
- explicit privilege;
- bounded streaming/capture;
- inspect/cancel operations;
- normalized results and cleanup status;
- idempotent start for repeated execution IDs;
- remote capability reporting.

### What changes later

```text
V0:
  Agent Runtime --in-process call--> LocalWorkstation --Linux--> machine

Later:
  Durable Core --authenticated RPC--> RemoteWorkstation executor --Linux--> replaceable machine
```

Provider routing, context, work state, Tool Registry, and client protocol remain above the port. Machine-local PIDs, absolute paths, and process handles never become domain dependencies.

### What the port deliberately excludes

The Workstation port does not own:

- Craxii identity enrollment;
- durable state storage;
- provider credentials;
- client sessions;
- tool schemas;
- model context;
- policy interpretation;
- canonical artifact retention;
- background-work scheduling.

Those concerns cross different trust/lifecycle boundaries and must not turn Workstation into a giant framework.

## Client/backend protocol

### Protocol principles

- Craxii owns the protocol; provider wire events never cross it.
- JSON envelopes carry `protocol_version=1`.
- Unknown required fields/types fail explicitly.
- Additive optional fields may be ignored by older clients.
- Authoritative mutations are HTTP commands.
- Durable server facts and ephemeral drafts use WebSocket delivery.
- Every durable event is replayable by cursor.
- Command acknowledgment is not dependent on a live WebSocket.

### Authentication

Every `/v1` HTTP request and WebSocket upgrade carries:

```text
Authorization: Bearer <random 256-bit device token>
```

The server hashes the random token with SHA-256 and compares hashes in constant time. A password-strength key derivation function is unnecessary for a uniformly random 256-bit token. The raw token lives in macOS Keychain and is provisioned out of band for V0.

The authenticated token resolves `device_id`. Revoked devices receive `401`. Provider API credentials are unrelated and never reach the client.

### HTTP endpoints

```text
GET  /health/live
GET  /health/ready
GET  /v1/bootstrap
POST /v1/conversations/{conversation_id}/messages
POST /v1/work-items/{work_id}/cancel
GET  /v1/events?after=<journal_offset>     WebSocket upgrade
```

Optional diagnostic/admin endpoints are out of scope and must not be exposed publicly without separate design.

### Protocol v1 closure and transport bounds

V0 request JSON rejects unknown fields and unsupported `protocol_version`; UUIDs are canonical
lowercase-hyphenated UUIDv7 values and replay cursors are canonical unsigned decimal. Response
envelopes are stable and may add optional fields that clients ignore. Every protected request has
exactly one bearer header: the scheme is ASCII case-insensitive, followed by exactly one ASCII
space and 64 lowercase hexadecimal characters, with no comma, tab, extra segment, or trailing
data. A fresh server UUIDv7 request ID is generated for every HTTP request and WebSocket upgrade;
inbound request-ID headers are never trusted or persisted.

Message JSON is limited to 512 KiB before decode and retains the 64 KiB decoded UTF-8 content
limit. Cancellation JSON is limited to 8 KiB. Active HTTP requests are bounded to 64, active
mutation requests to 16, and WebSocket connections to 32. Health, mutation, and bootstrap
timeouts are respectively two, ten, and thirty seconds; each replay page query is bounded to five
seconds, while WebSocket lifetime has no generic HTTP timeout. Responses are `no-store` and
`nosniff`; authorization is sensitive before tracing. Rust performs neither TLS, HSTS, permissive
CORS, nor forwarded-header trust.

After startup recovery, bootstrap/replay are admitted in `live_unready` or `ready`. Messages
require `ready`; cancellation is responsibility-reducing and remains admitted in `live_unready`
or `ready`. Mutations are rejected in `draining` or `fatal`. Production remains `live_unready`
until Stage 17 installs a real WorkRunner.

### Message command

Request:

```json
{
  "protocol_version": 1,
  "client_message_id": "<uuidv7>",
  "content": [
    { "type": "text", "text": "Inspect your machine..." }
  ]
}
```

The request also sends `Idempotency-Key: <same client_message_id>`. V0 rejects a mismatch.

New-command response: `202 Accepted`.

```json
{
  "protocol_version": 1,
  "message_id": "<uuidv7>",
  "work_id": "<uuidv7>",
  "work_state": "queued",
  "conversation_work_ordinal": 7,
  "committed_cursor": 143,
  "duplicate": false
}
```

An exact retry returns the stored logical IDs, ordinal, and cursor. It MAY return HTTP 200 or the original 202; V0 SHOULD preserve the original stored status and set `duplicate=true` in the regenerated convenience view without changing the stored domain response.

Validation errors commit no message/work and use a stable error envelope.

### Cancellation command

Request includes protocol version and a fresh client command/idempotency ID. Response identifies work, resulting state, committed cursor, duplicate status, and whether asynchronous cleanup remains.

Expected statuses:

- `202` for active work now `cancel_requested`;
- `200` for queued work cancelled immediately or already-terminal work;
- `404` unknown work;
- `409` idempotency conflict.

### Error envelope

```json
{
  "protocol_version": 1,
  "error": {
    "code": "idempotency_conflict",
    "message": "The idempotency key was already used for different command material.",
    "retryable": false,
    "request_id": "<opaque>"
  }
}
```

Errors contain no stack trace, provider response body, command output, secret, or database path.

### Bootstrap snapshot

`GET /v1/bootstrap` starts a SQLite read transaction and:

1. Reads the current maximum journal offset `H` inside that snapshot.
2. Reads Craxii identity/display metadata.
3. Reads the primary conversation and committed messages.
4. Reads queued/active/recent terminal work projections.
5. Reads unresolved interruption/outcome-unknown summaries.
6. Commits/releases the read transaction.
7. Returns the projection with `snapshot_cursor=H`.

Because all reads share one SQLite snapshot, returned state never includes a projection mutation committed after `H`.

Bootstrap does not include provider request bodies, raw tool output artifacts, secrets, or internal-only journal events.

The public bootstrap DTO contains exactly `protocol_version`, `snapshot_cursor`, `craxii`,
`primary_conversation`, `messages`, `work_items`, and `unresolved_outcomes`; it is not the internal
bootstrap type. Craxii exposes only ID, display name, and owner label. Conversation exposes ID,
kind, lifecycle, and creation time. Messages expose stable IDs, conversation sequence, role,
content, optional client-message/work IDs, and commit time. Work exposes stable IDs and ordinal,
all nine public Work states, trigger message, lifecycle times, optional safe terminal reason,
cleanup pending, and redacted tool summaries. Tool summaries expose only execution ID, tool name,
safe status/result class, timestamps, and outcome-unknown status. Closed unresolved warnings are
`provider_outcome_unknown`, `tool_outcome_unknown`, and `cleanup_unconfirmed` with only the IDs
needed for client presentation.

The transaction first reads journal head `H`, then every projection, and closes before JSON
serialization or network send. It returns messages in conversation sequence order, Work in work
ordinal order, tools in agent-step/tool-ordinal order, and warnings in work/tool order. It fails
non-partially with `bootstrap_limit_exceeded` above 2,048 messages, all active plus 512 recent
terminal Work, 2,048 tool summaries, 12 MiB source message JSON, or 16 MiB encoded JSON; it never
silently truncates.

### Durable WebSocket event envelope

```json
{
  "protocol_version": 1,
  "delivery_kind": "durable",
  "event_id": "<journal event uuid>",
  "cursor": 144,
  "event_type": "work.started",
  "conversation_id": "<uuid or null>",
  "work_id": "<uuid or null>",
  "recorded_at": "<UTC RFC3339>",
  "payload": {}
}
```

Protocol projection maps internal journal events to client-safe payloads. It may omit internal event types. Cursor progression is still based on global journal offsets; when internal events are skipped, the next delivered cursor may jump.

Client-visible durable events include:

- `message.accepted`;
- `work.queued`;
- `work.started`;
- `work.waiting_on_model` as a high-level progress state;
- `tool.execution_started` client projection;
- `tool.execution_finished` client projection;
- `work.cancel_requested`;
- `work.cancelled`;
- `work.completed`;
- `work.failed`;
- `work.interrupted`;
- `assistant.message_committed`;
- relevant `runtime.recovery_performed` summary.

`work.waiting_on_tool` is public. An internal `work.resumed`, if a later stage emits it, maps to
public `work.started` with `transition_kind="resumed"`. The exact allowlist also includes
`tool.execution_interrupted_before_dispatch`; tool dispatch maps to
`tool.execution_started`, while completed and outcome-unknown terminal facts map to the safe
finished projection. Initialization, runtime start/stop, model invocation, tool request/internal
arguments, artifacts/evidence, provider bodies, context manifests, filesystem/output data, and
internal runtime/attempt facts are never public. An unknown kind/version whose visibility cannot
be proven fails closed rather than being skipped.

Replay wire cursor `0` is the from-start sentinel; positive cursors wrap global journal offsets.
Negative, signed, padded, floating, junk, overflow, noncanonical, missing-required, and future
values are invalid. SQLite scans at most 128 underlying journal rows per page through a fixed
high-water, returning typed candidates, `scanned_through`, and `has_more`; scan progress crosses
filtered/internal rows and therefore can advance on an empty public page.

Internal event names and public event names may differ where redaction/aggregation requires it.

### Ephemeral draft envelope

```json
{
  "protocol_version": 1,
  "delivery_kind": "ephemeral",
  "event_id": "<ephemeral uuid>",
  "cursor": null,
  "event_type": "assistant.draft_delta",
  "conversation_id": "<uuid>",
  "work_id": "<uuid>",
  "draft_id": "<uuid>",
  "invocation_id": "<uuid>",
  "delta_sequence": 12,
  "payload": { "text": "..." }
}
```

Ephemeral event types are:

- `assistant.draft_started`;
- `assistant.draft_delta`;
- `assistant.draft_abandoned`;
- `sync.complete`;
- heartbeat/ping information.

Drafts are best effort. They are not written to the journal, not included in bootstrap, and not replayed after disconnect.

### Streaming and commit relationship

- A draft may appear before an assistant message exists.
- The eventual `assistant.message_committed` durable event contains the complete authoritative content.
- On commit, the client replaces the draft with the committed message by work ID/draft relationship.
- If an invocation fails, retries after exposed semantic output are disabled and `assistant.draft_abandoned` is emitted.
- If the socket disappears, no backend work state changes because of that fact.

### Replay and live handoff algorithm

After obtaining bootstrap cursor `H`, the client opens `/v1/events?after=H`.

The server performs:

1. Authenticate and validate `after` is nonnegative and not ahead of the current journal head.
2. Subscribe the connection to the in-process commit notification channel before reading replay.
3. Read current journal high-water `R`.
4. Query and send every client-visible durable event with `H < journal_offset <= R` in ascending offset order.
5. Set connection `last_sent_cursor=R`, even if some internal events were not projected.
6. Drain notification entries already received, discarding durable cursors `<= R`.
7. Send `sync.complete` with `through_cursor=R`.
8. Enter live mode.

In live mode, a notification is only a wakeup hint. On each wakeup or detected broadcast gap, the connection queries SQLite for all eligible events after `last_sent_cursor`, sends them in order, and advances. This makes broadcast-channel loss a latency event, not data loss.

Ephemeral drafts are delivered only after sync completion. A draft produced while a reconnecting client catches up may be missed; the eventual committed message remains durable.

### Reconnect behavior

The Mac client:

1. Retains its latest fully processed durable cursor in local client state.
2. On connection loss, marks drafts disconnected but does not mark work failed.
3. Retries command submissions only with the original idempotency key.
4. Fetches bootstrap when starting fresh, after a protocol mismatch, or when local projection is uncertain.
5. Reconnects WebSocket with the bootstrap/last cursor.
6. Applies durable events idempotently by event ID/cursor.
7. Discards all pre-disconnect drafts.
8. Uses committed assistant messages as final truth.

V0 never prunes journal events, so a valid old cursor remains replayable. Later retention will require snapshot epochs; that is deferred.

### Backpressure

- Each WebSocket connection has a bounded send queue.
- Durable events are never dropped silently. If a slow client exceeds the queue, close the socket with a retryable code; it will replay from its cursor.
- Ephemeral drafts may be coalesced or dropped under pressure.
- Maximum durable event payload is 256 KiB; larger evidence uses artifacts and summaries.
- One slow client cannot block journal commits or the agent loop.

The committed-cursor broadcaster has capacity 256 and is only a wakeup hint. Each connection
subscribes before reading high-water, rechecks durable storage every second, and repairs hint loss
or lag by scanning after its last scanned cursor. Each connection has an outbound queue of 16
frames, a 262,144-byte durable payload limit, a 270,336-byte encoded-frame limit, and a five-second
queue/send-stall limit. Sustained pressure closes `1013`; client application frames close `1008`,
controlled shutdown closes `1001`, and replay/storage invariant failure closes `1011`.

The 32-connection limit and transport shutdown drain count accepted pending upgrades as well as
active sockets. A connection subscribes to shutdown before its initial high-water read, and every
high-water read, replay-page fetch, frame send, `sync.complete` send, and live wait is
shutdown-aware. Registry ownership is removed only after the upgrade callback and any transferred
connection work have produced an observed completion outcome.

### Transport shutdown ordering

Transport mutation admission is separate from scheduler claim admission. Graceful shutdown
latches, marks nonfatal health draining, stops listener acceptance and new upgrades, closes
mutation admission and awaits explicit asynchronous quiescence, then closes claim admission and
awaits quiescence before committing `runtime.stopping`. The mutation section covers command
commit/no-commit and postcommit effect ownership, so no command event can commit after
`runtime.stopping`. Existing WebSockets then close `1001`; server and connection tasks are joined
before SQLite closes. Bootstrap binds the configured loopback listener before creating a runtime,
but does not serve application traffic until startup/recovery is coherent; bind failure creates no
RuntimeInstance. The Stage 10 shutdown controller remains the sole signal/shutdown authority.
The server execution task is observed by an explicit transport supervisor. Unexpected early
return, precise serve failure, or task panic marks fatal, requests that existing Stage 10
controller, and preserves the originating typed cause while transport ownership drains within the
controller's original deadline.

LocalWorkstation receives that exact original absolute Stage 10 deadline when execution admission
closes. For each already-owned execution its effective cleanup deadline is the earlier of the
execution request deadline and the Stage 10 deadline; TERM grace, KILL escalation, descendant and
cgroup proof, direct-child reap, drain joins, supervisor join, and terminal registry handoff MUST
NOT extend it. If proof cannot complete by the Stage 10 deadline, the supervisor performs the
strongest immediately available group/cgroup/direct-child kill action, reports cleanup uncertainty,
and still joins its nonblocking terminal handoff before bootstrap closes SQLite. Bootstrap reports
shutdown failure; it neither creates a later cleanup deadline nor leaves a supervisor able to use
runtime state after SQLite closure.

Repository checks for this surface are structural guards over method/path construction,
authentication placement, snapshot/replay source relationships, permanent test inventory, and
stage exclusions. They do not prove runtime behavior; deterministic Rust unit and integration
tests are the behavioral authority for atomic snapshots, replay progress, connection ownership,
and shutdown supervision.

## Native macOS client

### Technology and layers

```text
SwiftUI views
    |
    v
@MainActor ConversationStore / view models
    |
    +-- CraxiiHTTPClient
    +-- CraxiiEventStreamClient
    +-- ReconnectController
    +-- KeychainDeviceCredentialStore
```

AppKit is used only where SwiftUI lacks required native behavior. The backend protocol remains platform-neutral.

### Client responsibilities

The client owns:

- message composition;
- stable client message ID generation before submission;
- Keychain storage of device token;
- optimistic/pending display state;
- authoritative HTTP command retries;
- snapshot projection;
- cursor tracking and idempotent event application;
- draft rendering and replacement;
- queued/running/tool progress presentation;
- connection/reconnect status.

The client does not own:

- provider credentials;
- agent-loop iteration;
- context assembly;
- tool execution;
- work scheduling;
- canonical history;
- model selection;
- cancellation completion truth.

### Local state

V0 may keep the rendered projection in memory. If it persists a small cache, the cache MUST be disposable and namespaced by backend Craxii ID plus protocol version. Bootstrap/replay always wins.

The client should show:

- pending submission until HTTP acknowledgment;
- queued work after durable acknowledgment;
- active work and high-level tool progress;
- ephemeral assistant draft with a noncommitted visual treatment;
- committed assistant message;
- failed/cancelled/interrupted state distinctly;
- `outcome_unknown` warning without claiming a machine result.

### Reconnect backoff

Use bounded exponential backoff with jitter, reset after a stable connection. Network reconnect never resubmits a message under a new ID. The user may explicitly resend as new work only by creating a new message.

## Authentication and V0 security

### Must solve in V0

- TLS/WSS from Mac to proxy.
- Backend bound to loopback.
- Source-restricted security group.
- Random per-device bearer token stored in Keychain; only hash server-side.
- Server-side provider secret only.
- Backend process not permanently root.
- Explicit privilege field on every shell execution.
- Sanitized child environment and closed file descriptors.
- No production/customer/catastrophic credentials or data.
- Encrypted EBS and off-guest snapshots.
- No unauthenticated public command or WebSocket endpoint.
- Strict log redaction and no request/tool bodies by default.
- Request, timeout, output, loop, and process cleanup limits.
- An EC2 instance profile with no customer or snapshot-destruction authority.

### Provider credential handling

The OpenAI API key:

- is a dedicated development-project key;
- has a practical spend/rate limit and can be revoked independently;
- is loaded by systemd credentials or an equivalently root-owned secret file at process start;
- is wrapped in a Rust secret type whose `Debug`/`Display` output is redacted;
- is used only inside the OpenAI adapter;
- is never inserted into ContextPackage, tool environment, journal payload, artifact, protocol response, or tracing field;
- is rotated after suspected workstation compromise.

Because backend and tools share one root-reachable VM, V0 does not claim the key is protected from hostile root. Environment omission prevents routine accidental disclosure, not determined host compromise.

### Child-process secret exposure

The launcher MUST:

- clear inherited environment;
- set `FD_CLOEXEC`/close unrelated descriptors;
- provide null stdin unless specified;
- avoid command-line arguments containing backend secrets;
- avoid shell history and profile loading;
- never mount/copy provider credentials into a workspace;
- ensure tracing does not record the complete environment or raw command if it may contain user-entered sensitive data.

### Request protections

Tower middleware SHOULD provide:

- request IDs;
- authentication extraction;
- maximum HTTP body size;
- JSON content-type checks;
- ordinary command timeouts that do not apply to WebSocket lifetime;
- concurrency limits sufficient to prevent accidental local overload;
- redacted tracing spans.

Do not apply a generic response timeout to the WebSocket or an accepted background work item. HTTP message acceptance should remain a short database command.

### Can defer

- passkeys/OIDC user accounts;
- device enrollment UI;
- multi-device authorization policy;
- external Authority Service;
- federated workload identity;
- short-lived provider-native customer credentials;
- sandbox/trust realms;
- project VM isolation;
- prompt-injection resource policy;
- deterministic enforcement of “do not deploy”;
- production network segmentation;
- browser credential isolation;
- tamper-evident/WORM audit.

### Required seams for later authority

- immutable Craxii, work, workstation, workspace, tool-execution, and device identities;
- explicit authority decision before dispatch;
- requested and effective privilege;
- provider credentials confined to provider adapters;
- Workstation separate from the durable runtime;
- no customer authority attached to base instance identity;
- causation/correlation across work, model, tool, and artifacts.

## AWS deployment layout

### Filesystem paths

```text
/opt/craxii/
  releases/<version>/craxii-server    immutable release binary
  current -> releases/<version>       atomic active symlink

/etc/craxii/
  config.toml                         non-secret typed configuration
  caddy/Caddyfile                     TLS proxy configuration
  credentials/                        root-owned source for systemd credentials

/var/lib/craxii/
  db/craxii.sqlite3                   canonical V0 database
  artifacts/                          canonical/diagnostic artifact store
  backups/                            short-lived consistent backup staging
  locks/                              backend-owned local locks if needed

/srv/craxii/workspaces/
  primary/                            default project/workstation root

/home/craxii/
  .local/                             user-installed CLIs
  .cargo/                             Rust toolchain state
  .cache/                             regenerable user caches

/var/cache/craxii/                    backend regenerable caches
/run/craxii/                          runtime sockets/PID-like ephemeral files
```

Backend state, artifacts, and workspaces MUST not be intermingled under one user home directory. This is operational clarity, not security containment.

### Ownership and modes

- Release directories are root-owned and not writable by normal backend execution.
- The active binary is executed as `craxii`.
- `/var/lib/craxii` is writable by the backend user and mode-restricted from unrelated users.
- Workspace directories are owned by `craxii`.
- Credential sources are root-owned and provided to the service without global environment variables.
- The `craxii` user has an explicit noninteractive administrative path; therefore it can ultimately change these files. That is accepted V0 authority.

### Configuration

Typed startup configuration includes:

- Craxii/backend bind address and port;
- public base URL/protocol version;
- database and artifact roots;
- primary workspace logical/resolved roots;
- workstation ID/generation source;
- enabled model targets and default target;
- OpenAI endpoint and secret credential name;
- provider timeouts/retry bounds;
- work/model/tool limits;
- output/capture limits;
- shell path and environment allowlist;
- authentication token-hash source or device bootstrap path;
- tracing format/filter;
- shutdown grace period.

Configuration is validated before database mutation. Unknown configuration keys SHOULD fail startup to catch misspellings. Secrets are referenced by logical credential name, not embedded in the config file.

### systemd service expectations

The `craxii.service` unit MUST or SHOULD specify:

```text
User=craxii
Group=craxii
ExecStart=/opt/craxii/current/craxii-server --config /etc/craxii/config.toml
WorkingDirectory=/var/lib/craxii
Restart=on-failure
RestartSec=2s
KillSignal=SIGTERM
KillMode=control-group
TimeoutStopSec=30s
UMask=0077
Delegate=yes                         for per-execution cgroups
```

It SHOULD set reasonable file/process limits and start-order dependencies on mounted data volume and network availability.

It MUST NOT:

- run Cargo;
- run the backend permanently as root;
- set `NoNewPrivileges=true`, because that would prevent the agreed autonomous admin path;
- use `ProtectHome=true` or a filesystem sandbox that hides Craxii's workstation;
- preserve provider secrets in a globally readable environment file;
- restart infinitely with no rate limit after a deterministic configuration/schema failure.

Systemd hardening flags that do not conflict with workstation ownership may be added only after verifying tool behavior.

### Caddy expectations

Caddy:

- listens on 443 for the configured hostname;
- obtains/renews a trusted certificate;
- proxies to loopback Axum;
- preserves WebSocket upgrade and reasonable idle behavior;
- sets forwarded scheme/address headers from trusted local input;
- limits request bodies consistently with the backend;
- does not log authorization headers, bodies, or WebSocket payloads;
- has no access to SQLite, workspaces, or provider key.

The backend trusts proxy forwarding headers only from loopback.

### Release deployment

V0 release deployment is manual but repeatable:

1. Build and test the release artifact for x86-64 Linux.
2. Record version, Git revision, Cargo lock hash, and checksum.
3. Take a pre-migration data-volume snapshot when schema changes.
4. Copy binary to a new immutable versioned release directory.
5. Verify checksum on host.
6. Run a schema compatibility/preflight command that makes no external side effect.
7. Atomically switch `/opt/craxii/current`.
8. Restart `craxii.service`.
9. Verify readiness, recovery summary, database schema, and provider/tool smoke tests.
10. Keep at least one previous compatible binary.

Rolling deployment, zero downtime, and automatic rollback are out of scope. A binary rollback must never open a database schema it does not support.

## EBS and backup strategy

### Volume requirements

- EBS encryption MUST be enabled.
- The data volume MUST use `DeleteOnTermination=false`.
- Filesystem and mount configuration MUST survive reboot.
- Disk usage alerts or at minimum structured measurements MUST cover database, WAL, artifacts, workspaces, Docker, and package caches.
- Running out of disk is a fatal readiness/operation risk; tool execution should receive a structured failure rather than corrupting state.

### Snapshot policy

Minimum V0 policy:

- automated daily data-volume snapshots;
- retain at least 14 daily recovery points;
- on-demand snapshot before schema migration and benchmark release;
- snapshot automation owned by AWS Data Lifecycle Manager/AWS Backup, not an instance role;
- instance has no permission to delete snapshots;
- snapshot IDs and timestamps recorded in deployment operations, not model context.

An EBS snapshot is crash-consistent. SQLite WAL/FULL and recovery make it usable, but a separately produced SQLite-consistent backup is preferred before important releases.

### SQLite-consistent backup

Use SQLite's online backup mechanism or a vetted equivalent that includes committed WAL state. Do not use a raw copy of the main DB file.

The backup process:

1. Requests/creates a consistent destination database through SQLite APIs.
2. Verifies `quick_check` on the destination.
3. Records source journal head and backup hash/size.
4. Stores it in backup staging on the data volume.
5. Takes or waits for an off-guest snapshot containing it.
6. Removes old staging copies only after snapshot confirmation.

### Restore test

Before V0 is declared done:

1. Restore a snapshot to a new volume.
2. Attach it to a test EC2 instance with no provider/customer authority.
3. Start the compatible backend against a copied/restored path.
4. Verify database integrity and journal/projection consistency.
5. Verify canonical artifact hashes.
6. Reconstruct the machine-inspection conversation.
7. Confirm the follow-up can be answered using a scripted provider or the real dev provider.
8. Record recovery point and recovery time.

V0 does not claim a backup until this succeeds once.

## Observability and evaluation evidence

### Separation of records

```text
Journal / canonical tables
  what Craxii durably did and observed

tracing / journald
  how the Rust process behaved while doing it

metrics derived from rows/traces
  aggregate evidence for architecture decisions
```

Tracing must never be required to reconstruct a conversation or work result.

### Tracing format

- Use structured JSON tracing in the EC2 service.
- Use human-readable pretty output only in local development.
- Include service version, runtime instance, and subsystem.
- Keep correlation IDs in spans, not high-cardinality metric labels.
- Default level is `info`; provider/body/process output remains excluded at `debug` unless a consciously enabled redacted diagnostic capture is used.

### Required spans

```text
service_startup
database_migration
startup_recovery
http_request
client_command
websocket_connection
event_replay
work_queue_wait
work_execution
context_assembly
model_selection
model_invocation_attempt
provider_stream
tool_execution_service
workstation_read_file
workstation_execute
process_cleanup
artifact_write
journal_transaction
sqlite_checkpoint
backup
```

Correlation fields:

- request ID;
- device ID only where necessary and redacted/pseudonymous;
- Craxii/conversation/work IDs;
- invocation/logical invocation IDs;
- tool/execution IDs;
- runtime and workstation generation;
- journal offset range.

Do not record full message content, commands, environment, stdout/stderr, authorization headers, provider keys, or artifact bytes as span fields.

### Work-item measurements

Record or derive:

- queue wait milliseconds;
- time to first durable progress;
- time to first draft;
- time to committed answer;
- total duration;
- terminal state and reason;
- agent-loop step count;
- model logical invocation and attempt count;
- tool execution count by tool;
- cancellation requested/completed latency;
- interruption/outcome-unknown count;
- context-limit and loop-limit failures.

### Model invocation measurements

Every attempt records:

- provider and model;
- target config version and selection reason;
- logical invocation/attempt number;
- context manifest and request hashes;
- request bytes;
- input tokens;
- cached input tokens where reported;
- output tokens;
- reasoning tokens where reported;
- total tokens;
- total latency;
- first-byte latency;
- first semantic output latency;
- stop/incomplete reason;
- tool-call count;
- provider request and response IDs;
- retry classification and delay;
- whether draft output escaped;
- normalized error and provider status code;
- optional cost computed later from a separately versioned price table, never guessed from current internet pricing.

### Context measurements

Every manifest records the measurements already specified plus:

- prior-work versus active-work share;
- tool-schema share;
- largest message/tool result;
- estimator error against reported input tokens;
- linear growth per conversation work ordinal;
- provider cache utilization where reported.

### Tool measurements

Every execution records:

- tool/version/schema;
- requested/effective privilege;
- validation and authority outcome;
- time from model response commit to dispatch intent;
- queue/dispatch/start latency;
- execution duration;
- result kind, exit code, or signal;
- timeout/cancellation;
- stdout/stderr observed/captured/inline bytes;
- truncation;
- artifact write latency/bytes;
- process-tree termination phases;
- cleanup confirmation;
- outcome unknown;
- workstation generation.

### Storage and recovery measurements

- journal transaction count and p50/p95/max latency during tests;
- `SQLITE_BUSY` count;
- WAL bytes and checkpoint latency;
- DB and artifact bytes;
- free disk bytes;
- backup age and last verified restore;
- startup recovery duration;
- number of old-runtime work/attempts classified;
- integrity-check failures;
- orphan artifact count.

### Protocol measurements

- accepted commands;
- exact deduplications;
- idempotency conflicts;
- command commit-to-response latency;
- WebSocket connects/disconnects;
- reconnect attempts;
- replay event count and cursor lag;
- slow-client disconnects;
- dropped/coalesced ephemeral draft events;
- auth failures;
- bootstrap duration and snapshot message count.

V0 does not require Prometheus, OpenTelemetry collectors, or an evaluation platform. Structured tables plus tracing are sufficient if all required fields are captured and can be queried.

## Failure taxonomy

### Normalized error envelope

Every subsystem converts implementation-specific failures into a stable, dependency-neutral domain category plus redacted detail:

```text
NormalizedError
  category
  code
  retryability = never | bounded | user_action | operator_action
  certainty = definite | outcome_unknown
  safe_message
  source_status?         redacted provider/OS code
  internal_detail?       tracing only
```

The fields are private. `NormalizedError` retains no raw source error, and `std::error::Error::source()` returns `None`. `Display` emits only `safe_message`. Its custom `Debug` includes only category, code, retryability, certainty, safe message, and safe source status. Serialization uses the exact field names `category`, `code`, `retryability`, `certainty`, `safe_message`, and optional `source_status`, omitting `source_status` when absent. Display, Debug, serialization, and `source()` MUST NOT expose internal detail, a cause, a backtrace, a provider body, a path, or other raw input. Semantic equality compares every safe field, including source status, and ignores internal detail. Stage 3 does not require `Hash`.

Retryability is advisory classification only and never authorizes execution, replay, or automatic tool retry. `bounded` does not imply idempotency; actual retry count, backoff, and policy belong to later owning stages. Certainty is orthogonal to category, code, and retryability and MUST NOT be derived by matching error text. Validation failures are always `definite`. `outcome_unknown` is legal when an external action may have crossed its side-effect boundary but its terminal outcome is not known; retry and recovery behavior remain later-owned.

The complete generic stable-code vocabulary owned by Stage 3 is `domain_validation` plus the fourteen category literals below: `authentication_error`, `client_protocol_error`, `idempotency_error`, `storage_error`, `state_conflict`, `context_error`, `model_selection_error`, `provider_error`, `tool_validation_error`, `authority_error`, `workstation_error`, `artifact_error`, `cancellation_error`, and `internal_invariant_error`. Later leaf codes such as timeout, context-limit, provider-exhaustion, or unknown-tool distinctions remain deferred to their owning implementation stages and MUST NOT be added to the generic Stage 3 vocabulary early.

`ErrorCode` is an opaque allowlisted stable-code value, not a public arbitrary string and not a closed exhaustive enum. It serializes as an approved lowercase ASCII snake-case string of at most 64 ASCII bytes. It has no public unchecked constructor, `From<String>`, or arbitrary adapter-supplied-code path. Deserialization accepts only the explicitly registered Stage 3 constants above; it rejects unknown, uppercase, and otherwise unapproved strings. A later owning stage may add an explicit stable constant and extend the allowlist, and callers MUST NOT depend on exhaustive enum matching.

`SafeMessage` is an allowlisted approved static message value with no arbitrary public `String` or `&str` constructor. It contains no user content, paths, provider bodies, SQL, commands, stdout/stderr, raw adapter text, or secret, token, or header material. Localization and final client wording remain later-owned.

`InternalDetail` is trace-only sanitized diagnostic metadata. Construction is crate-private and accepts only approved already-sanitized static or closed diagnostic values; it MUST NOT be built from `error.to_string()`. It implements none of `Serialize`, `Deserialize`, `Display`, or `Debug`, is not returned through `source()`, and has no public raw getter. It never contains a raw path, content, provider body, SQL, command, output, token, backtrace, or rejected input.

### Structured source status

`SourceStatus` is an optional structured and redacted origin status with exactly two Stage 3 variants:

| Kind | Field and bound | Canonical serialized form |
| --- | --- | --- |
| `provider_http` | integer `code`, `100..=599` | `{"kind":"provider_http","code":429}` |
| `os_errno` | integer `code`, `1..=i32::MAX` | `{"kind":"os_errno","code":13}` |

The exact field names are `kind` and `code`, and the kind literals are exact lowercase strings. Unknown kinds, invalid numeric bounds, and additional fields are rejected. Provider response bodies and textual status/reason strings, OS error messages, and filesystem paths are forbidden. Craxii's own future public-API HTTP status is not source status; Stage 11 owns that transport projection. Process exit codes and signals remain structured execution evidence. Raw provider-native string codes are deferred to the owning provider stage and require an explicit later durable contract. Source status participates in `NormalizedError` semantic equality and is safe for Display, Debug, and Serde because it contains only these approved structured numeric fields.

### Client validation projection

`DomainValidationError` remains the precise local constructor/validation error. There MUST NOT be `From<DomainValidationError> for NormalizedError` or any equivalent context-free automatic conversion. At a client-input or client-protocol boundary only, the explicit named `NormalizedError::from_client_validation(...)` mapper produces exactly:

```text
category       = client_protocol_error
code           = domain_validation
retryability   = never
certainty      = definite
safe_message   = The supplied value is invalid.
source_status  = None
internal_detail = closed sanitized DomainValidationKind diagnostic only
```

Rejected raw input is never retained. The same validation error may map differently when it originated in persisted corrupt data, provider/tool data, or an internal trusted invariant, so context determines category.

### Categories

| Category | Examples | Domain consequence |
| --- | --- | --- |
| `authentication_error` | invalid/revoked device token | Reject command; no domain write |
| `client_protocol_error` | invalid JSON/version/body size | Reject command; no work |
| `idempotency_error` | same key, different material | Conflict; no work |
| `storage_error` | disk full, busy timeout, failed fsync | Do not claim commit; may make service unready |
| `state_conflict` | stale state version, cancellation race | Reload and follow winner |
| `context_error` | corrupt source, context limit | Definite work failure unless storage inconsistency |
| `model_selection_error` | no capable target | Definite work failure |
| `provider_error` | 429, 5xx, auth, malformed stream | Bounded retry or definite failure by classification |
| `tool_validation_error` | malformed arguments, unknown tool | Observed structured tool result |
| `authority_error` | denied privilege/tool | Observed structured tool result |
| `workstation_error` | file/process/cleanup failure | Tool result or unknown outcome |
| `artifact_error` | capture/rename/hash failure | Do not commit terminal result referencing missing bytes |
| `cancellation_error` | process tree not confirmed dead | Tool unknown, work interrupted |
| `internal_invariant_error` | journal/projection disagreement | Fail readiness/stop affected work; never guess |

The category vocabulary is exactly these fourteen literals. Generic Stage 3 constructors use the category literal as their code, except the client validation mapper above, and use these fixed safe messages and conservative classifications:

| Generic category | Retryability | Certainty | Safe message |
| --- | --- | --- | --- |
| `authentication_error` | `user_action` | `definite` | `Authentication is required.` |
| `client_protocol_error` | `never` | `definite` | `The request is invalid.` |
| `idempotency_error` | `user_action` | `definite` | `The request conflicts with an earlier request.` |
| `storage_error` | `operator_action` | `definite` unless a later owning boundary explicitly supplies otherwise | `A storage operation failed.` |
| `state_conflict` | `bounded` | `definite` | `The requested operation conflicts with the current state.` |
| `context_error` | `never` | `definite` | `The requested context cannot be processed.` |
| `model_selection_error` | `operator_action` | `definite` | `No suitable model is currently available.` |
| `provider_error` | generic unknown defaults to `never`; an explicit controlled classification may select `bounded` | caller explicitly supplies certainty where request-send state matters | `The model provider request failed.` |
| `tool_validation_error` | `never` | `definite` | `The tool request is invalid.` |
| `authority_error` | `never` | `definite` | `The requested operation is not permitted.` |
| `workstation_error` | generic unknown defaults to `never` | caller explicitly supplies certainty where dispatch state matters | `The workstation operation failed.` |
| `artifact_error` | `operator_action` | caller may explicitly supply certainty for side-effect-boundary cases | `The artifact operation failed.` |
| `cancellation_error` | `never` | caller explicitly supplies certainty | `The operation could not be confirmed as cancelled.` |
| `internal_invariant_error` | `operator_action` | `definite` | `An internal consistency error occurred.` |

For state conflicts, `bounded` means re-evaluation may be allowed, not automatic replay. Provider and workstation mappings accept caller-chosen certainty rather than infer it from text. Generic unknown provider and workstation failures remain `never`; provider `bounded` classification is possible only through an explicit controlled constructor. Storage and artifact failures remain their own categories, and an unknown adapter failure does not become an internal invariant.

`internal_invariant_error` is reserved for Craxii-trusted impossible or contradictory state: journal/projection contradiction, impossible lifecycle combinations, unclassifiable recovery state, unsupported required durable semantics, or programmer/task failure that invalidates trusted state assumptions. It is not a fallback for provider, storage, artifact, tool/client validation, stale state conflict, or OS/workstation failures.

The generic Stage 3 cancellation error has category and code `cancellation_error`, retryability `never`, and caller-supplied certainty. Unconfirmed cleanup may be `outcome_unknown`. Successful cancellation is not necessarily an error; later lifecycle stages own that state. The later leaf code `cancelled` is not part of Stage 3.

### Definite versus unknown

The architecture distinguishes:

- **Definite failure before dispatch:** no external side effect began; safe to report failure.
- **Observed terminal outcome:** side effect ran and result was observed, including nonzero/timeout/cancel.
- **Outcome unknown:** dispatch may have occurred but Craxii cannot observe or prove its terminal state.

Only the last category prohibits automatic repetition because repeating could duplicate or compound the side effect.

## Crash windows

### Message acceptance

| Crash point | Durable state after SQLite recovery | Required behavior |
| --- | --- | --- |
| Before transaction | Nothing | Client retries same ID |
| During transaction before commit | Nothing visible | Client retries same ID |
| After commit before HTTP response | Message/work/events/idempotency response all exist | Retry returns original IDs; scheduler runs once |
| After response before scheduler wakeup | Work queued | DB scan eventually claims it; notification loss irrelevant |

### Model invocation

| Crash point | Durable record | Recovery |
| --- | --- | --- |
| Before invocation intent commit | No provider attempt | Work may be active; mark interrupted in V0 |
| After intent commit before HTTP send | Attempt `requesting` | Conservative provider outcome unknown/interrupted; no auto resume |
| After request send before response bytes | Attempt `requesting` | Provider outcome unknown; work interrupted |
| During stream after draft | Attempt `streaming`, draft ephemeral | Abandon draft; invocation/work interrupted |
| After normalized response commit before tool dispatch | Complete invocation evidence | Work can be classified from committed state; V0 startup still does not silently resume active loop; mark work interrupted unless a specific deterministic reconciliation completes it |

Model output alone has no workstation side effect. V0 still avoids hidden automatic loop resumption so the first recovery semantics remain simple and auditable.

### Tool execution

| Crash point | Durable record | Recovery |
| --- | --- | --- |
| Before `tool.execution_requested` commit | No execution | Work interrupted if runtime died |
| After requested, before dispatch intent | `requested` | `interrupted_before_dispatch`; no side effect claimed |
| After dispatch intent, before spawn | `dispatching` | `outcome_unknown` conservatively |
| After spawn, before output | `dispatching` | `outcome_unknown`; systemd/cgroup kills ordinary children on restart |
| After command exits, before artifact finalize | `dispatching` | `outcome_unknown`; observed result was not durable |
| After artifact rename, before DB commit | Orphan artifact + `dispatching` | `outcome_unknown`; later orphan cleanup |
| After terminal DB commit, before event delivery | Complete result/event | Replay sends it; no re-execution |

### Final answer

| Crash point | Durable state | Behavior |
| --- | --- | --- |
| During draft | No assistant message | Draft disappears/abandoned |
| Before final transaction commit | No terminal message/work completion | Recovery interrupts work |
| After commit before WebSocket event | Message and completed work | Replay delivers exact message |
| After delivery before client cursor persistence | Message and completed work | Client may receive duplicate event and deduplicates by cursor/event ID |

### Cancellation

| Crash point | Recovery |
| --- | --- |
| Before cancel command commit | Retry command |
| After `cancel_requested`, before runtime observes | Recovery marks old active work interrupted unless cleanup can be proven |
| While SIGTERM/SIGKILL cleanup | Tool outcome unknown if cgroup empty cannot be confirmed |
| After cleanup/terminal commit before response | Idempotent cancel retry returns terminal state |

## Sequence diagrams

### Successful message, tool loop, and final answer

```text
Mac Client       HTTP/API       CommandSvc      SQLite       Scheduler
    | POST message   |              |              |              |
    |--------------->| authenticate |              |              |
    |                |------------->| BEGIN        |              |
    |                |              | message+work+events+idem    |
    |                |              |------------->|              |
    |                |              | COMMIT       |              |
    |                |<-------------| 202 IDs+cursor              |
    |<---------------|              |              | notify       |
    |                |              |              |------------->|
    |                |              |              | claim+event  |
    |                |              |              |<-------------|

Scheduler       AgentLoop      Context/Model     OpenAI       ToolSvc
    | own task       |                |              |              |
    |-------------->| select target  |              |              |
    |                |-------------->| manifest     |              |
    |                | persist invocation intent -> SQLite         |
    |                |-------------->| POST /responses             |
    |                |               |------------->|              |
    |                |<-- ephemeral text/tool stream |              |
    |                | persist completed ordered response          |
    |                |--------------------------------------------->|
    |                |               |              | persist requested
    |                |               |              | persist dispatch intent

ToolSvc         Handler       LocalWorkstation       Linux       SQLite
    |               |                |                 |            |
    |-------------->| typed input    |                 |            |
    |               |--------------->| execute         |            |
    |               |                |---------------->|            |
    |               |                | drain/wait/kill/reap         |
    |               |<---------------| observed result |            |
    | artifact finalize ------------------------------------------->|
    | terminal tool outcome+event -------------------------------->|
    |<-------------- canonical bounded result                       |

AgentLoop       Context/Model     OpenAI          SQLite       WebSocket
    | next context    |               |              |              |
    |---------------->| second call   |              |              |
    |                 |-------------->|              |              |
    |<----------------| final ordered response       |              |
    | final assistant + work.completed transaction->|              |
    |                                                | commit notify|
    |                                                |------------->|
    |                                                              |-->
    |                                          committed answer to Mac
```

### Second message while first work is active

```text
Conversation ordinal:       N                         N+1

Work N           Mac Client          CommandSvc/DB          Work N+1
  | shell running    |                    |                    |
  |                  | "Also ..." HTTP   |                    |
  |                  |------------------->|                    |
  |                  |                    | commit message     |
  |                  |                    | create queued ---->|
  |                  |<-------------------| 202 queued         |
  |                  |                    |                    |
  | next model call  |                    |                    |
  | context includes ordinals <= N and active N outputs        |
  | context explicitly excludes N+1 message                    |
  | completes        |                    |                    |
  |------------------| terminal commit    |                    |
  |                                       | scheduler claims ->|
  |                                       |       now N+1 runs |
```

### Snapshot and reconnect without race

```text
Mac Client           Bootstrap API        SQLite       Event Service
    | GET bootstrap       |                  |              |
    |-------------------->| BEGIN READ       |              |
    |                     | read head H      |              |
    |                     | read projections|              |
    |                     | END READ         |              |
    |<--------------------| snapshot, H      |              |
    |                                                    commit H+1
    | WS after=H ------------------------------------------->|
    |                     subscribe wakeups                  |
    |                     read head R >= H+1                 |
    |<-------------------- replay (H, R] --------------------|
    |<-------------------- sync.complete through R ----------|
    |                                                    live commit
    |<------------------------------------------------ durable event

If the live notification channel drops an item, Event Service queries SQLite
after its last cursor. SQLite, not the notification channel, closes the gap.
```

### Crash during non-idempotent shell command

```text
AgentLoop        ToolSvc         SQLite       LocalWorkstation      systemd
    | tool call      |              |                 |                |
    |-------------->| requested tx |                 |                |
    |                |------------->| COMMIT          |                |
    |                | dispatch tx  |                 |                |
    |                |------------->| COMMIT intent   |                |
    |                |------------------------------->| spawn command  |
    |                |              |                 | side effects   |
    X backend killed |              |                 |                |
                                                                     kill cgroup
                                                                     restart backend
New Runtime       Recovery        SQLite
    |                 | inspect dispatching row         |
    |---------------->|-------------------------------> |
    |                 | tool = outcome_unknown          |
    |                 | work = interrupted              |
    |                 | append recovery events, COMMIT  |
    |                 | no automatic execute() call     |
```

### Cancellation of a running command

```text
Mac        CommandSvc/DB      AgentLoop       Workstation       Process tree
 | POST cancel   |               |                 |                 |
 |-------------->| commit cancel_requested        |                 |
 |<--------------| 202 cleanup pending            |                 |
 |                               | observe token   |                 |
 |                               |---------------->| SIGTERM group   |
 |                               |                 |---------------->|
 |                               |                 | wait 5s         |
 |                               |                 | SIGKILL remains |
 |                               |                 | reap/cgroup empty
 |                               |<----------------| cleanup confirmed
 |                    commit tool cancelled + work cancelled         |
 |<---------------- durable events through WebSocket ----------------|
```

### Startup recovery

```text
systemd          New backend        SQLite/Artifacts       Scheduler
   | ExecStart        |                     |                  |
   |----------------->| config/schema       |                  |
   |                  | quick_check         |                  |
   |                  |-------------------->|                  |
   |                  | create runtime      |                  |
   |                  | inspect nonterminal |                  |
   |                  | classify attempts   |                  |
   |                  | append recovery tx  |                  |
   |                  | verify artifacts    |                  |
   |                  |--------------------------------------->|
   |                  | ready only after recovery + scheduler |
```

### Future durable-core extraction

```text
V0
--
Mac -> HTTPS/WSS -> Rust backend + SQLite + LocalWorkstation on one VM

Pre-V1
------
Mac -> Durable Core
         |-- external state/artifact/identity/authority
         +-- Workstation port -> RemoteWorkstation executor on replaceable VM

The agent loop, tool definitions, context policy, work IDs, causal journal model,
and client protocol remain conceptually above the workstation boundary.
```

## Failure and recovery policies

### Provider failure

- Classify status/error in the adapter.
- Persist each attempt before request and after observed terminal error.
- Retry only under the model retry policy.
- Respect cancellation during request/backoff.
- Do not accept a partial tool call.
- If a retry begins, use a new attempt ID and preserve the prior failed row.
- If all attempts fail, transition work to `failed(provider_exhausted)`.
- A provider refusal is normalized output, not a transport failure.

### Tool failure

- Unknown tool and invalid arguments are observed tool results.
- File-not-found and permission-denied are observed tool results.
- Shell nonzero/signaled/timeout with confirmed cleanup are observed tool results.
- Handler panic or artifact persistence failure is an internal failure; never fabricate a model-facing ordinary result if canonical evidence did not commit.
- No tool call is automatically retried.
- Unknown outcome interrupts the work immediately.

### Storage failure

- Before commit: return/propagate failure and do not publish an event.
- After SQLite reports commit success: treat state as durable even if HTTP/WebSocket delivery fails.
- Disk-full or I/O integrity errors should mark readiness false and stop claims.
- A journal/projection mismatch is an invariant failure requiring operator action.
- Artifact finalization failure prevents the terminal tool/model record from referencing the artifact.

### Child process on backend death

The service-level cgroup and `KillMode=control-group` should terminate ordinary descendants when systemd stops/restarts the unit. This limits continued execution but does not resolve whether side effects occurred before death. Durable classification remains outcome unknown.

An OS-managed service intentionally started by a root command may survive backend death. The journal proves only that the starting command was dispatched/observed, not the service's later health. Future process/service tools will close that gap.

## Crash-injection test plan

Crash injection is a first-class architecture test, not optional chaos polish.

### Failpoint mechanism

The implementation SHOULD expose test-only failpoints at named boundaries. In integration tests, a controller starts the backend subprocess, enables one failpoint, submits deterministic work, waits for a marker, then uses `SIGKILL`. Failpoints never compile into release behavior unless disabled and unreachable.

Required failpoints:

```text
after_message_transaction_commit
after_work_claim_commit
after_context_manifest_commit
after_model_intent_commit
after_first_provider_delta
after_model_response_commit
after_tool_requested_commit
after_tool_dispatch_intent_commit
after_tool_process_spawn
after_tool_process_exit_before_outcome_commit
after_artifact_rename_before_db_commit
after_assistant_message_commit
after_cancel_requested_commit
during_graceful_shutdown
```

Stage 10 activates exactly `after_message_transaction_commit`, `after_work_claim_commit`,
`after_cancel_requested_commit`, and `during_graceful_shutdown`. They are development/test-only and
the release compile guard remains authoritative. Message and active-cancellation failpoints fire
only for a newly committed transaction, before hint/caller delivery; replay, queued direct
cancellation, and terminal no-op do not re-fire them. Claim fires after its commit and before runner
registration. Shutdown fires after claim admission closes, the in-flight claim section quiesces,
and `runtime.stopping` commits, but before drain or classification. No additional public failpoint
name is authorized.

### Required assertions

For each crash test:

- database `quick_check` succeeds;
- journal stream sequences and projections agree;
- no duplicate message/work/tool execution exists;
- no terminal event claims an unobserved result;
- old-runtime active state is classified exactly once;
- duplicate client command returns the original response;
- queued later work remains causally excluded from the crashed work;
- process group/cgroup is empty after systemd recovery unless an intentional OS service was started;
- replay delivers every committed public event after the test client's cursor;
- no provider/tool retry occurs contrary to policy.

### Side-effect marker test

Use a test-only shell command confined to a disposable workspace:

```text
append a unique execution ID to a marker file
fsync marker
sleep long enough for crash injection
```

Kill after dispatch intent or spawn. After recovery:

- zero or one marker occurrence is possible depending on crash timing;
- tool state is `outcome_unknown` either way when no outcome committed;
- there is no second marker caused by automatic retry;
- the work item is interrupted.

This demonstrates honest ambiguity rather than pretending exactly-once side effects.

## Testing strategy

### Domain unit tests

- every legal/illegal work transition;
- state-version guards;
- one-message-one-work mapping;
- event payload serialization/version compatibility;
- causation/correlation assignment;
- request-hash stability;
- context eligibility with queued later messages;
- context ordering across tool loops;
- interruption synthetic status rendering;
- ordered model-output terminal decision;
- provider/tool retry classification;
- privilege decision recording;
- normalized error mapping.

### SQLite integration tests

Use real SQLite temporary files with WAL/FULL:

- migrations from empty database;
- bootstrap idempotence;
- concurrent duplicate message submissions;
- ordinal allocation under concurrency;
- active-work partial unique constraint;
- atomic message/work/events/command response;
- stream sequence allocation;
- journal replay versus current projections;
- crash/reopen WAL recovery;
- busy timeout behavior;
- snapshot read/high-water consistency;
- artifact-before-reference ordering;
- recovery classification transactions.

In-memory SQLite is insufficient for WAL/reopen/crash tests.

### Scripted model provider

A deterministic provider adapter is required for tests. Given expected context/tool results, it emits scripted ordered output and stream events. It supports:

- text then tool call;
- multiple tool calls;
- malformed/partial arguments;
- refusal;
- transient pre-output failure then success;
- failure after draft;
- timeout;
- unknown provider item;
- final machine-inspection answer.

This validates the provider boundary without consuming API capacity or tying correctness tests to model nondeterminism.

### Workstation tests

Linux integration tests verify:

- relative/absolute cwd resolution;
- UTF-8, binary, missing, and oversized reads;
- clean environment excludes sentinel backend secret;
- user and administrative effective identity;
- independent stdout/stderr drain under high output;
- exit code versus signal versus spawn failure;
- timeout escalation;
- cancellation;
- child/grandchild process cleanup;
- cgroup empty verification;
- output capture/inline truncation and hashes;
- no implicit cwd/environment persistence between calls.

Process/cgroup tests run on Ubuntu, not solely macOS CI.

### Provider contract tests

OpenAI adapter tests use recorded redacted fixtures and local HTTP fixtures for:

- ordered Responses output items;
- streaming text and tool-argument deltas;
- encrypted/opaque reasoning continuation;
- usage details;
- incomplete/failed responses;
- request/response IDs;
- 429/5xx/retry guidance;
- unknown event handling;
- `store=false`, disabled truncation, and no provider conversation dependency;
- tool schema/call ID/result round trip.

At least one live development API smoke test validates current wire compatibility before the release benchmark.

### Protocol tests

- auth reject/accept/revoke;
- body and message limits;
- exact duplicate and conflict behavior;
- bootstrap cursor/projection atomicity;
- replay from every offset around one transaction;
- internal skipped-event cursor jumps;
- live notification loss repaired from SQLite;
- slow client disconnect and replay;
- draft abandonment and committed replacement;
- reconnect after final commit before delivery;
- unknown protocol version.

### Native client tests

- stable ID reuse across HTTP retry;
- optimistic message reconciles to server message ID;
- queued and active rendering;
- cursor/event deduplication;
- all drafts discarded on reconnect;
- committed message replaces draft;
- interruption and outcome-unknown presentation;
- Keychain credential access and revoked-token response;
- background/foreground socket transitions.

## Acceptance tests and pass/fail criteria

### Acceptance A: real end-to-end machine inspection

Pass only if:

- Mac native app, EC2 backend, OpenAI, and LocalWorkstation are all real;
- exactly one user message/work exists;
- at least one real tool executes on Ubuntu;
- tool intent precedes tool start in durable order;
- result follows observation;
- final answer matches OS, architecture, cwd, and Git version;
- model/tool/context usage rows are complete;
- no provider-native type appears in public protocol or domain modules.

### Acceptance B: completed restart continuity

Pass only if:

- completed history survives SIGKILL and systemd restart;
- new runtime ID is observed;
- recovery completes before readiness;
- reconnect replays without duplicate message;
- follow-up answer is correct from reconstructed context;
- provider conversation state can be disabled/deleted without changing correctness.

### Acceptance C: tool observed failure

Pass only if:

- nonexistent file or nonzero command yields structured observed result;
- backend remains live;
- model can explain/recover;
- event and tool row distinguish failure kind;
- work ends honestly.

### Acceptance D: ambiguous side effect

Pass only if:

- backend is killed after dispatch intent;
- recovery marks tool outcome unknown and work interrupted;
- no automatic duplicate dispatch occurs;
- client receives interruption via replay;
- journal never records false success/failure.

### Acceptance E: exact duplicate command

Pass only if simultaneous identical submissions create one message/work and both callers receive the same logical result.

### Acceptance F: queued causal isolation

Pass only if a second message accepted during a delayed first tool call is visible in the UI, queued in SQLite, and absent from every first-work context manifest.

### Acceptance G: cancellation

Pass only if cancellation of a long foreground command terminates/reaps its process tree, commits terminal cancellation, starts no further model/tool call, and remains correct after reconnect.

### Acceptance H: reconnect race

Pass only if an assistant message committed between bootstrap and WebSocket sync appears exactly once in the client projection.

### Acceptance I: context limit

Pass only if a deliberately small configured model limit causes explicit `context_limit_exceeded`, no provider auto-truncation, and no hidden omission from the manifest.

### Acceptance J: restore

Pass only if an off-guest snapshot/consistent backup restores on a replacement test VM and reconstructs canonical conversation/work/artifact evidence.

## Implementation order

Implementation proceeds by correctness spine, not by visible UI first.

### Milestone 0: repository and decision scaffolding

- Rust workspace and Cargo lockfile;
- backend module skeleton matching ownership boundaries;
- typed config and build metadata;
- tracing/redaction bootstrap;
- dependency decision records;
- protocol/domain type test harness.

Gate: service starts locally with no model/tool/state behavior hidden in handlers.

### Milestone 1: canonical IDs, state machine, and SQLite

- typed IDs;
- work state machine;
- SQLx migrations for core tables;
- WAL/FULL configuration;
- event append/stream sequence allocation;
- projection transactions;
- deterministic projector tests;
- initial Craxii/conversation/workstation/workspace bootstrap.

Gate: journal replay equals projections and illegal transitions fail.

### Milestone 2: idempotent responsibility spine

- device auth fixture;
- HTTP message command;
- atomic message/work/input/events/idempotency transaction;
- scheduler FIFO claim;
- cancellation command/state;
- replay cursor and bootstrap snapshot;
- concurrent duplicate tests.

Gate: headless service accepts, queues, deduplicates, cancels, restarts, and replays deterministic work without a model.

### Milestone 3: Workstation and tool evidence

- Workstation port;
- LocalWorkstation capabilities/read/execute/inspect/cancel;
- process group/cgroup cleanup;
- local artifact store;
- Tool Registry and Tool Execution Service;
- `read_file` and `run_shell`;
- requested/dispatch/outcome transactions;
- crash injection around tool boundaries.

Gate: scripted work performs real bounded Ubuntu commands with no duplicate unknown side effect.

### Milestone 4: context and scripted agent loop

- Context Assembler eligibility/manifests;
- selected-target token budget interface;
- canonical model request/ordered response;
- explicit agent loop and limits;
- deterministic scripted provider;
- tool loop and terminal message commit.

Gate: the full responsibility spine passes with deterministic model output, including queued causal isolation and crash tests.

### Milestone 5: OpenAI adapter

- Reqwest client and secret type;
- Responses request/stream decoder;
- ordered output normalization;
- stateless reasoning continuation handling;
- attempt/retry/usage records;
- redacted fixtures and live smoke test.

Gate: headless canonical inspection task passes against the real provider.

### Milestone 6: live event delivery

- durable event projection;
- WebSocket replay/live handoff;
- ephemeral drafts;
- backpressure and reconnect tests.

Gate: simulated clients cannot miss or duplicate committed facts across disconnects.

### Milestone 7: native macOS client

- Keychain token;
- HTTP command client;
- bootstrap/event stream;
- conversation/work/draft UI;
- retry/reconnect projection;
- cancellation control.

Gate: native client passes duplicate, queue, draft, reconnect, and interruption tests against local/backend fixtures.

### Milestone 8: EC2 operationalization

- Ubuntu x86-64 EC2;
- encrypted root/data EBS layout;
- `craxii` user and autonomous sudo path;
- systemd/Caddy;
- typed config/systemd credentials;
- snapshot automation and restore rehearsal;
- repeatable release deployment.

Gate: all real benchmark components run on the target topology.

### Milestone 9: release benchmark

Run Acceptance A through J, preserve result evidence, and record baseline metrics. V0.0.01 is not done if only the happy path passes.

## Migration seams toward pre-V1

### Durable core extraction

The later durable core can move off the workstation in this order:

1. Implement an external artifact backend behind `ArtifactStore`; keep artifact IDs/hashes.
2. Implement external canonical State Store with equivalent command transactions and cursor semantics.
3. Move Craxii identity/ownership and recovery metadata to that durable core.
4. Implement authenticated `RemoteWorkstation` using existing execution IDs and logical workspaces.
5. Move provider credentials and policy evaluation into an external Authority Service.
6. Reprovision the workstation from generation metadata and restore only selected workspace state.

The agent loop should require composition changes, not a rewrite, if boundaries are respected.

### State-store seam

Application code uses intent-specific transaction methods, not raw SQL or a giant generic repository. Examples of port-level operations:

- accept message and create work;
- claim next work;
- transition work with event;
- begin/finish invocation;
- begin dispatch/finish tool;
- commit assistant completion;
- create bootstrap snapshot at cursor;
- replay public events after cursor;
- recover old-runtime state.

This contract maps to SQLite now and a remote transactional database later. It does not pretend SQLite and distributed storage are identical; implementation-specific consistency remains inside the adapter.

### Artifact seam

Domain records retain:

- artifact ID;
- content hash and length;
- logical retention class;
- storage backend/key opaque to clients;
- provenance.

Moving bytes to S3 changes the adapter and backup policy, not tool/model/journal identities.

### Authority seam

The V0 local allow decision already receives durable identities and requested operation context. Later it can return:

- provider/workload credential handle;
- scoped resource envelope;
- machine-enforced prohibitions;
- expiration and revocation epoch;
- remote audit correlation.

Tool handlers still receive an authorized execution context rather than raw renewable roots.

### Model-provider seam

Adding a second provider requires:

- new adapter and typed native options;
- capability/target config;
- canonical input/output mapping;
- provider-specific token estimator/stream fixtures;
- selection policy configuration;
- context eligibility rules for its opaque continuation items.

It must not require changing work, tool, client, or journal primitive semantics.

### Context and memory seam

V0.0.02 may add:

- compaction artifacts with provenance;
- recent verbatim tail;
- retrieval/search projections;
- memory assertions with source links;
- supersession/correction semantics;
- model-specific context policies.

The Context Assembler consumes these as additional typed source candidates and records them in manifests. Raw journal/history remains retained and re-derivable.

### Background work seam

Future schedules/events create work items with nonconversation triggers and nullable conversation ID. The scheduler can add global/project concurrency without changing the work lifecycle. The user-facing client may project this work into the one relationship without exposing internal workers.

V0 does not implement any trigger other than a user message.

## Explicit architecture debt

### Accepted V0 debt

1. **Canonical state shares the workstation trust domain.** Root compromise can corrupt SQLite, artifacts, and workspaces.
2. **Provider secret shares the host.** Environment sanitization does not protect it from hostile root.
3. **No true workstation replacement.** Reattachment/restore is manual and identity is only as durable as local state/backups.
4. **Full-history context grows linearly.** It will fail once the selected model cannot fit it.
5. **No in-flight resume.** Active work becomes interrupted even when a deterministic continuation might be possible.
6. **Ambiguous side effects require human/model follow-up.** V0 does not reconcile external systems automatically.
7. **One active work item per conversation.** Independent responsibilities wait FIFO.
8. **No steering.** A corrective user message cannot alter active work except through explicit cancel.
9. **No durable process sessions.** Generic background daemons are unsupported; OS services are not tracked by the runtime.
10. **Local artifact retention is manual.** No S3, lifecycle policy, or remote integrity replica.
11. **One real provider.** Abstraction quality is tested only by a scripted adapter plus OpenAI.
12. **Development authority policy.** Natural-language constraints are prompt-level and audit-only, not machine-enforced.
13. **One failure domain/availability zone.** No high availability or distributed consensus.
14. **Bearer device token provisioning is manual.** No account recovery or device enrollment UX.

### Debt that must not leak into permanent contracts

- no absolute local path in client/model domain identifiers;
- no EC2/volume ID as Craxii identity;
- no provider conversation ID as history identity;
- no local PID as execution identity;
- no SQLx type in domain/application public contracts;
- no OpenAI response struct in the journal or client protocol;
- no tool handler direct journal write;
- no `Command` or `std::fs` use in agent-loop/context code;
- no WebSocket connection as command acknowledgment;
- no implicit all-history query that ignores work ordinals;
- no automatic retry hidden inside Workstation.

## What must remain replaceable

| Component | V0 implementation | Replacement expectation |
| --- | --- | --- |
| Model provider | OpenAI Responses adapter | Other provider adapters without work/tool rewrite |
| Model target policy | configured default | Evaluation/capability/cost routing later |
| Context policy | naive full history | compaction/retrieval/memory policies |
| Workstation | LocalWorkstation | authenticated RemoteWorkstation |
| State adapter | SQLx/SQLite | external durable store/control core |
| Artifact backend | local EBS | S3/object storage |
| Authority evaluator | local development allow | external Authority Service/policy compiler |
| TLS edge | Caddy | load balancer/private gateway/native TLS |
| Client | macOS SwiftUI | iOS/Android/Windows against same protocol |
| Event delivery wakeup | in-process channel | another notification mechanism; journal cursor stays authoritative |

## What must remain canonical

Across replacements, these meanings must survive:

- immutable `craxii_id`;
- accepted user message identity/content;
- work identity, input relationships, causal order, and lifecycle;
- intent-before-side-effect and observed-outcome-after ordering;
- uncertainty when no terminal outcome was observed;
- provider/model selection and invocation evidence;
- ordered model output and tool-call/result pairing;
- context provenance manifests;
- tool identity, arguments, privilege, workspace, and outcome evidence;
- artifact identity/hash/provenance;
- idempotent client command result;
- durable client replay order;
- correction/supersession by append, not historical mutation.

Physical SQLite pages, EBS IDs, Rust structs, OpenAI IDs, and local paths are not canonical meanings.

## What V0 intentionally does not solve

V0 does not claim:

- that Craxii remembers indefinitely once full history no longer fits;
- that root on its workstation cannot alter Craxii;
- that a killed shell command did or did not partially mutate the machine;
- that snapshots have zero data loss;
- that the provider did not retain/bill a cancelled request beyond configured API behavior;
- that model proposals are safe or correct;
- that local admin actions respect user prohibitions through hard policy;
- that a workspace can safely hold production credentials;
- that work continues when the EC2 instance/EBS/AWS account is unavailable;
- that multiple devices/users have mature authorization;
- that tool output is searchable or semantically remembered;
- that every service the shell starts is supervised by Craxii.

Honesty about these limits is part of the architecture.

## Definition of done

V0.0.01 is complete only when all of the following are true:

### Product behavior

- The native macOS app connects to one persistent Craxii identity.
- It submits user work idempotently and shows queued/active/terminal state.
- Craxii performs real tool calls on its Ubuntu workstation.
- Final committed answers stream/replay correctly.
- The canonical machine-inspection and follow-up tasks pass.

### Durability and correctness

- Message/work creation is atomic.
- Journal/projections reconstruct deterministically.
- Every model and tool external action has persisted intent first.
- Every claimed outcome was observed and persisted after the action.
- Ambiguous non-idempotent execution becomes outcome unknown with no auto retry.
- Duplicate client commands produce exactly one work item.
- Later queued input never leaks into active context.
- Process kill/restart and reconnect tests pass.
- Off-guest restore succeeds once.

### AI-runtime quality

- Target selection precedes rendering.
- Every invocation has a context manifest.
- Full-history context fails explicitly at its limit.
- Ordered mixed output items are handled.
- Provider-native state is optional optimization/evidence, not canonical conversation state.
- Tool calls are complete and validated before dispatch.
- Agent-loop and retry limits prevent infinite work.
- Baseline token, context, latency, cost-input, and tool-reliability evidence is queryable.

### Workstation behavior

- All model-facing machine access crosses Workstation.
- `read_file` handles text, missing, binary, and size cases honestly.
- `run_shell` uses explicit cwd, sanitized environment, bounded capture, timeout, privilege, and process-tree cleanup.
- Administrative package/Docker/service operations are possible.
- Backend itself is non-root.
- No production/catastrophic authority exists on the VM.

### Protocol and client

- HTTP command acknowledgment survives lost responses.
- Bootstrap and cursor replay close the reconnect race.
- Durable events are never silently dropped.
- Draft events are visibly noncanonical and safely abandoned.
- Slow clients cannot block work or persistence.
- Device token remains in Keychain and provider key remains server-side.

### Operations

- systemd restarts the backend and kills ordinary descendant process trees.
- Caddy terminates HTTPS/WSS with redacted logs.
- SQLite runs WAL/FULL and passes integrity checks.
- Encrypted EBS and automated off-guest snapshots are active.
- Release deployment and compatible rollback procedure are documented and rehearsed.
- Required traces/metrics exist without secrets/content leakage.

If a happy-path demo passes while any required crash, duplicate, causal-isolation, cancellation, or restore test fails, V0.0.01 is not done.

## Final architecture diagram

```text
┌──────────────────────────────────────────────────────────────────────┐
│ Native macOS client                                                  │
│ Swift + SwiftUI (+ AppKit where needed)                              │
│                                                                      │
│ ConversationStore                                                    │
│   ├── HTTP commands: message / cancel / bootstrap                    │
│   ├── WebSocket: durable replay + ephemeral drafts                   │
│   ├── durable cursor projection                                      │
│   └── device token in Keychain                                       │
└───────────────────────────────┬──────────────────────────────────────┘
                                │ HTTPS / WSS
                                ▼
┌──────────────────────────────────────────────────────────────────────┐
│ AWS EC2 · Ubuntu 24.04 LTS · x86-64 · one V0 trust/failure domain   │
│                                                                      │
│  ┌──────────────────────┐                                            │
│  │ Caddy                │ TLS, WebSocket proxy, no Craxii semantics  │
│  └──────────┬───────────┘                                            │
│             │ loopback HTTP                                          │
│             ▼                                                        │
│  ┌────────────────────────────────────────────────────────────────┐  │
│  │ craxii-server · Rust · Tokio · User=craxii                    │  │
│  │                                                                │  │
│  │ Axum/Tower protocol adapters                                   │  │
│  │   ├── Command Service ── idempotent atomic mutations           │  │
│  │   └── Event Delivery ── SQLite cursor replay + live drafts     │  │
│  │                                                                │  │
│  │ Durable scheduler                                              │  │
│  │   └── one active work_item per conversation, FIFO              │  │
│  │            │                                                   │  │
│  │            ▼                                                   │  │
│  │ Explicit Agent Loop                                            │  │
│  │   ├── Model Selection                                          │  │
│  │   ├── Context Assembler + exact manifest                       │  │
│  │   ├── Model Gateway                                            │  │
│  │   │     └── OpenAI Responses adapter ── Reqwest ──► OpenAI    │  │
│  │   │          ordered items, stateless, store=false             │  │
│  │   └── Tool Execution Service                                   │  │
│  │          ├── intent / authority seam / outcome                 │  │
│  │          └── Tool Registry                                     │  │
│  │                ├── read_file                                   │  │
│  │                └── run_shell                                   │  │
│  │                       │                                        │  │
│  │                       ▼                                        │  │
│  │              Workstation port                                 │  │
│  │                       │                                        │  │
│  │                       ▼                                        │  │
│  │              LocalWorkstation                                 │  │
│  │                files · Bash · sudo · cgroups                   │  │
│  │                                                                │  │
│  │ State Store                                                    │  │
│  │   └── SQLx ── SQLite WAL/FULL                                 │  │
│  │         ├── append-only journal                                │  │
│  │         ├── current-state projections                          │  │
│  │         ├── model/tool attempt evidence                        │  │
│  │         └── replay cursor                                      │  │
│  │                                                                │  │
│  │ Artifact Store ── local content-addressed evidence             │  │
│  │ tracing ── structured diagnostics, separate from journal       │  │
│  └────────────────────────────────────────────────────────────────┘  │
│                                                                      │
│  Encrypted EBS data                                                  │
│    ├── /var/lib/craxii       canonical V0 DB + evidence artifacts   │
│    ├── /srv/craxii/workspaces durable workstation/project state     │
│    └── /home/craxii          development environment                │
│                                                                      │
│  systemd                                                            │
│    ├── restarts backend                                             │
│    ├── controls service cgroup                                      │
│    └── kills ordinary descendants on backend stop                   │
└───────────────────────────────────┬──────────────────────────────────┘
                                    │ snapshots controlled off-guest
                                    ▼
                         ┌────────────────────────┐
                         │ AWS EBS snapshot plane │
                         │ recoverable backup     │
                         └────────────────────────┘

Later extraction:

  external durable identity/state/artifacts/authority
                         │
                         ▼
               same Agent Runtime boundaries
                         │
                         ▼
               RemoteWorkstation on a replaceable VM
```

## Source-of-truth hierarchy

If implementation and documents conflict, use:

1. This document's product invariants and normative behavior.
2. Explicit later architecture amendments approved by the project owner.
3. Versioned protocol schema and database migrations created from this architecture.
4. Domain type/state-machine contracts.
5. Adapter and application implementation details.
6. Comments, tests, and operational scripts.

Tests are evidence, not permission to contradict architecture. If an implementation discovery invalidates a decision, update the architecture through an explicit challenge/change proposal before normalizing the deviation in code.

## Architecture change standard

A proposed change to a foundational boundary must state:

- current decision;
- observed problem and evidence;
- proposed behavior;
- alternatives considered;
- effect on product intent;
- state/protocol/schema migration;
- new failure and security modes;
- effect on V0 scope and benchmark;
- reversibility;
- whether it blocks implementation.

Foundational boundaries include:

- identity and durable IDs;
- work/conversation relationship;
- journal event envelope;
- context eligibility/manifests;
- provider abstraction and ordered output;
- intent/outcome ordering;
- Workstation interface;
- privilege/trust model;
- client commands and cursor replay;
- canonical/backup/ephemeral classification.

## References

These primary references support specific external-system semantics used by this architecture:

- [OpenAI Responses API: create a model response](https://developers.openai.com/api/reference/cli/resources/responses/methods/create)
- [SQLite write-ahead logging](https://www.sqlite.org/wal.html)
- [SQLite synchronous pragma](https://www.sqlite.org/pragma.html#pragma_synchronous)
- [Tokio child process lifecycle](https://docs.rs/tokio/latest/tokio/process/struct.Child.html)
- [Axum framework documentation](https://docs.rs/axum/latest/axum/)
- [AWS EBS snapshots](https://docs.aws.amazon.com/ebs/latest/userguide/ebs-snapshots.html)

The configured OpenAI model ID, pricing, account limits, and availability are runtime/deployment facts and must be reverified against official provider documentation during implementation. They are intentionally not frozen into this architecture.
