# Stage 27 adversarial pre-reboot audit

This is the repository-side audit frozen before the next real-host run. It does not declare Stage
27 complete and does not authorize a reboot. Host-dependent claims remain explicitly unproven
until `verify-production-host.sh --pre-reboot` reports its two required terminal markers.

## Composition matrix

The columns are: **A** deployed `craxii-server.service`; **B** Stage 27 `run_live_test`; **C**
Stage 27 `run_live_unit_test`; **D** ordinary Cargo/unit tests; **E** `Stage18Harness` scripted
composition; **F** production `LocalWorkstation` as constructed by the server. “Child” means the
model-controlled shell/reader after the fixed launcher.

| Dimension | A | B | C | D | E | F | Difference classification and concrete failure mode |
|---|---|---|---|---|---|---|---|
| UID | parent `craxii-server`; child `craxii` | test parent `craxii-server`; child `craxii` | same | developer | caller unless Linux profile; child follows configured workstation | parent service; child `craxii` | B/C are equivalent only after launcher proof. D/E can false-pass identity code unless the Linux profile is selected. |
| GID | matching primary groups | explicit matching GID | explicit matching GID | developer | disposable roots normally caller-owned | fixed service/workstation groups | **TEST CAN FALSE-PASS** if an ordinary harness is treated as an identity proof. Stage 27 selects the Linux profile. |
| Supplementary groups | service result is host-observed; launcher clears child to empty | parent `--clear-groups`; child empty | same | host shell groups | ordinary harness inherits caller; Linux profile uses launcher | launcher calls `setgroups(0)` | B/C intentionally stricter for the parent; child semantics are equivalent. Actual service group set remains a real-service observation. |
| Effective/permitted/inheritable/ambient capabilities | exactly `CAP_KILL`; bounded transition set is KILL/SETGID/SETUID/SETPCAP | KILL+SYS_ADMIN so an outside verifier can cross delegation | KILL inside delegated root | normally none | depends on profile | parent service boundary; child all zero | B can **FALSE-PASS/FALSE-FAIL** for parent cgroup operations. The divergence is recorded, never presented as service parity. Child zero sets are independently required. |
| Capability bounding set | minimal transition set | host/test bounding set | host/test bounding set | host login set | previously unexamined | launcher now drops every child bound capability | Retaining the bounding set was a **SECURITY_BOUNDARY** defect. `no_new_privs` reduced exploitability but did not satisfy “all capabilities cleared.” |
| `no_new_privs` | parent must be 0 so setuid launcher works; child 1 | child 1 | child 1 | host-dependent | only Linux launcher profile proves it | launcher sets and child reports 1 | Intentional parent difference with equivalent child semantics. Setting it on the unit would break the selected transition architecture. |
| Service cgroup membership | `/system.slice/craxii-server.service` | verifier remains outside; child migrates into delegated execution subtree | test parent first enters execution root | none | ordinary harness none; Linux profile configured | parent service plus per-execution UUID child | B cannot by itself prove service authority. Production startup now creates a UUID cgroup and places its launcher probe there before readiness; B remains the longer cleanup-path proof. |
| Delegated authority and parent cgroup | `Delegate=yes`; backend owns its subtree | SYS_ADMIN crosses from an unrelated cgroup | parent already inside delegated root | none | selected profile only | backend creates UUID child below configured root | The B divergence is explicit. The real CAP_KILL-only service now proves create/place/execute/remove during startup; C approximates ancestry for crash fixtures. |
| Process/session/process group | systemd owns main process; each execution calls `setsid` | launched child must report PID=PGID=SID | same LocalWorkstation path | test-specific | production adapter when Linux profile selected | new session and process group plus cgroup | Child proof now rejects a launcher path that failed before Bash or skipped `setsid`. Cgroup, not PGID alone, remains cleanup truth. |
| CWD / actual working directory | `/var/lib/craxii`; child pinned to resolved workspace fd | runner now `cd /var/lib/craxii`; child asserts canonical workspace | `/var/lib/craxii` | checkout/package root | disposable workspace | configured persistent workspace | Previous B/C values could **FALSE-FAIL/FALSE-PASS** relative-path and startup behavior. Disposable workspace storage is intentional, not a persistence proof. |
| HOME/USER/LOGNAME | service `/var/lib/craxii` identity; child fixed `/home/craxii`, `craxii`, `craxii` | exact clean parent values; exact child values | same | developer | ordinary caller values | launcher fixed values | Intentional clean-room test difference with equivalent child semantics. |
| PATH / executable paths | unit and launcher fixed absolute paths | clean fixed parent PATH; production launcher | same | developer toolchain PATH | fixture target paths unless Linux profile | fixed launcher, `/bin/bash`, adjacent reader | D/E can false-pass PATH discovery. Stage 27 uses absolute executable paths and asserts installed release identity. |
| Inherited environment | systemd manager environment plus credential directory metadata; forbidden provider/AWS names absent | `env -i` with explicit safe variables | same | developer environment | scripted provider is network-free | launcher `env_clear` plus fixed safe set | B/C cannot prove the parent's exact systemd environment. Real service environment names are now captured without values; child forbidden names are asserted. |
| systemd credentials/provider visibility | backend receives `LoadCredential`; contents are never audited | no credential directory or provider credential | same | may inherit developer env unless test sanitizes | scripted provider has no credential | backend only; launcher child has neither env metadata nor readable file | B/C intentionally lack credentials. They prove containment, not backend credential loading. Metadata and negative access are real-service checks. |
| Proc visibility | unit process visible subject to kernel policy | verifier root orchestrator can inspect; child cannot read backend environ | similar | host-dependent | macOS/non-Linux differs | child self-status readable, backend environ denied by DAC | **TEST CAN FALSE-PASS** if root observations are confused with child access. Probes execute the negative read as the child. |
| IMDS reachability | systemd `IPAddressDeny` applies to service cgroup descendants | child is in that exact cgroup subtree and safely probes denial | not probed | host network | scripted/no network | production child inherits cgroup filter | Static unit text alone was insufficient. B now combines effective unit equality, exact cgroup membership, executable curl presence, and IPv4/IPv6 denial. Service-owned spawn remains host-only. |
| Filesystem ACL behavior | persistent default ACL gives server effective read-only access to model files | disposable root uses modes; persistent sentinel separately checks exact ACL/mask | same | host filesystem | mode-based disposable relaxation | real persistent default/access ACL | Disposable roots are an **INTENTIONAL TEST DIFFERENCE** only for execution. They cannot prove production ACLs. Raw mode and ACL-effective permissions are recorded separately. |
| Umask | 0077 | now 0077 and child asserts inherited 0077 before fixture changes it | now 0077 | developer | harness default previously host-dependent | inherited through launcher | Previous B/C could false-pass file-creation behavior. Files needing server fixture access change umask only after proving inheritance. |
| RLIMIT_NOFILE / RLIMIT_NPROC / RLIMIT_CORE | 65536 / 16384 / 0 | now explicitly identical and child asserts | same | host defaults | host defaults unless Stage 27 runner | inherited by launcher child | Previous live tests did not exercise production exhaustion boundaries and could false-pass/fail. A nonzero core limit could persist backend credential memory. |
| Signal authority | service has `CAP_KILL`; systemd owns control group | KILL+SYS_ADMIN parent; child none | KILL parent | caller | mocks or selected Linux path | backend can signal other-UID child; child cannot regain authority | B's extra authority is test-only. Signal, timeout, cancellation, and service-restart results now require distinct canonical evidence. |
| Launcher/cgroup root | installed immutable launcher and fixed service path | same installed launcher/root | same | usually direct shell/mock | explicit Linux profile only | startup-validated absolute launcher/root | A wrong fixture path can no longer count as Stage 27 success; host runner passes both explicitly and Bash must prove it ran. |
| Mount namespace/bind layout | host namespace, EBS ext4 plus three binds | same host namespace but disposable workspace | same | developer FS | disposable temp root | production persistent paths | B does not prove persistence merely by sharing the namespace. Evidence captures UUID, FSROOT and bind roots before/after restart/reboot. No private mount namespace is configured. |
| Umask/ACL interaction | 0077 plus default ACL mask produces raw 0640/effective server r-- for sentinel | fixture mode relaxation after inherited-umask proof | same | host-specific | not production ACL | kernel ACL inheritance | Raw `0600` expectation was a known false-fail. The contract is effective ACL access and derived raw mode. |
| systemd lifecycle ownership | Restart=on-failure, KillMode=control-group, TimeoutStopSec=30s | verifier survives outside the service | parent is killed if service root is killed | Cargo owns process | harness owns server tasks | production systemd owns backend | Synthetic restart validates cgroup cleanup but not active work hosted inside the real backend. That last invariant remains host-only. |
| Shutdown/restart path | SIGTERM -> runtime 10s drain; systemd 30s ceiling; failure restart after 2s | coordinated `systemctl restart` | none | test cancellation tokens | harness shutdown grace 5s | real configured 10s | Harness 5s is a documented test acceleration, not proof of the production deadline. Actual unit/runtime values and graceful durable stop are compared. |
| Start rate/main PID replacement | systemd 5 starts/60s; real MainPID | observes but does not own | none | none | none | service restart replaces runtime/PID | **UNPROVEN** under repeated failures; one normal restart and reboot are in scope. Snapshot prevents a symlink-only false-pass by checking `/proc/MainPID/exe` and runtime revision. |

## Cross-layer terminal evidence contract

`ExecutionResult` uses non-null Rust booleans. Ambiguity entered at the public persistence DTO,
where `ToolTerminalOutcome.timed_out` and `.cancelled` are optional because pre-dispatch and unknown
states need NULL. Production construction is centralized in `tool_execution_service.rs`; the final
SQLite adapter normalizes definite post-dispatch completions, validates the complete tuple, writes
the same normalized values, and rereads them in consistency checks. Migration V5 now makes the
same distinction durable at the schema boundary.

### Current 0199 NULL divergence

The 0199 adapter computes normalized local `timed_out`/`cancelled` values, validates those values,
and binds those same locals into the SQL update. `finish_run_shell` also constructs both flags from
non-null `ExecutionResult` booleans. Therefore the audited 0199 source has no production
`finish_run_shell` route by which a newly committed definite success can retain NULL. The earlier
regression proved only a new synthetic adapter request; it did not rewrite a V4 row that was
already durable, and the old release process did not bind the installed binary bytes to the named
Git revision. The exact repository-level divergence is thus **request-local normalization versus
schema-permitted durable V4 state, compounded by unproven release and host-test provenance**. The earliest layer
where `None` can survive is the V4 `tool_executions` row itself. V5 repairs compatible history and
rejects every future invalid tuple; the commit-bound installed manifest makes the next host run
decide whether the observed row was historical or was produced by bytes other than the claimed
0199 source. The verifier also now requires its checkout to equal the deployment commit exactly,
and the live test asserts its embedded Git revision, so a later test-only checkout cannot silently
stand in for the deployed composition. Claiming a more specific live cause without that host
evidence would be speculation.

| Semantic observation | dispatched | started | result/state | exit | signal | timed_out | cancelled | cleanup | certainty/work terminal |
|---|---:|---:|---|---|---|---|---|---|---|
| unknown tool | no | no | unknown_tool/completed | NULL | NULL | NULL | NULL | NULL | definite; work continues or terminalizes through loop policy |
| validation rejection | no | no | validation_rejection/completed | NULL | NULL | NULL | NULL | NULL | definite |
| authority denial | no | no | authority_denial/completed with deny snapshot | NULL | NULL | NULL | NULL | NULL | definite |
| file/preparation error before intent | no | no | file_error/completed | NULL | NULL | NULL | NULL | NULL | definite |
| cancellation before dispatch | no | no | cancellation/completed or interrupted_before_dispatch | NULL | NULL | NULL | NULL | NULL | definite; work cancelled |
| handler rejection after durable intent but before handoff | yes | no | validation_rejection/completed | NULL | NULL | 0 | 0 | NULL | definite |
| spawn failure before child execution | yes | no | spawn_failure/completed | NULL | NULL | 0 | 0 | NULL | definite |
| file read success | yes | yes (logical handoff) | success/completed | NULL | NULL | 0 | 0 | NULL | definite |
| shell exit 0 | yes | yes | success/completed | 0 | NULL | 0 | 0 | 1 | definite |
| shell nonzero | yes | yes | process_exit/completed | nonzero | NULL | 0 | 0 | 1 | definite |
| signal termination | yes | yes | signal_termination/completed | NULL | positive | 0 | 0 | 1 | definite |
| timeout | yes | yes | timeout/completed | class-dependent NULL | optional observed signal | 1 | 0 | 1 | definite only after cleanup |
| cancellation after intent/handoff | yes | possibly no/yes | cancellation/completed | NULL | optional observed signal | 0 | 1 | 1 | definite; work cancelled |
| shutdown with observed cleanup | yes | possibly yes | definite terminal class | class-specific | class-specific | explicit 0/1 | explicit 0/1 | 1 | work interrupted/stopped per shutdown owner |
| cleanup unconfirmed | yes | unknown | outcome_unknown | NULL | NULL | NULL | NULL | 0 | outcome_unknown; work interrupted; never redispatch |
| handler panic before durable handoff | no or durable intent with proof of no handoff | as observed | pre-dispatch definite or completed rejection | class-specific | class-specific | N/A or explicit | N/A or explicit | as applicable | definite only with proof |
| handler panic after possible handoff | yes | unknown | outcome_unknown | NULL | NULL | NULL | NULL | 0 | interrupted; never redispatch |
| backend crash after intent before observation | yes | unknown | recovered outcome_unknown | NULL | NULL | NULL | NULL | 0 until cleanup proof | interrupted; never redispatch |
| process already gone with wait identity pinned | yes | yes | observed exit/signal if wait status exists | exact | exact | 0 | 0 | composite cleanup | definite |
| ambiguous disappearance/PID only missing | yes | unknown | outcome_unknown | NULL | NULL | NULL | NULL | 0 | interrupted; PID absence alone is insufficient |
| startup recovery after process loss | yes | persisted-dependent | outcome_unknown unless complete result was already committed | NULL | NULL | NULL | NULL | cleanup result | idempotent recovery; no side-effect retry |

Additional field invariants:

- `requested_cwd`, requested privilege, timeout and output policy are frozen on the requested row;
  `resolved_cwd`, effective privilege, allow decision and intent time appear atomically at dispatch.
- `started_at` is an observation, not a synonym for dispatch. Spawn failure may be dispatched with
  no start. `completed_at` is mandatory for every terminal state.
- Stream counts are all-null or a complete tuple satisfying observed >= captured >= inline and
  omitted = observed - inline. Artifact links and hashes are validated before the DB transaction;
  a publish-without-row is retained as a reported orphan, never silently deleted.
- `result_kind`, exit/signal/interruption flags and start state are validated as one tuple. The read
  side rejects rather than normalizes invalid stored state.
- Every terminal work projection clears both current-attempt columns; V5 evidence capture counts
  any violation. Journal terminal evidence and projection updates share one SQLite transaction.

Model invocation uses the same pattern: durable requesting intent precedes provider dispatch;
streaming and semantic-output observations are persisted; V4's cross-column constraint separates
reported/unavailable usage and exact certainty/error/billing ambiguity. Ambiguous post-dispatch
provider outcomes become `provider_outcome_unknown`, are billing-ambiguous, interrupt work, and are
not retried. Transient retries are limited to classifications proving no semantic output and carry
explicit retry ancestry/delay.

Work lifecycle is a closed state machine:
`queued -> running -> waiting_on_model|waiting_on_tool -> running`, with `cancel_requested` as a
controlled branch and only `completed|failed|cancelled|interrupted` terminal. Current attempt and
runtime ownership checks are optimistic-transaction preconditions. Runtime startup opens and
checks SQLite, loads identity, classifies/reconciles old runtime/work/model/tool state, verifies
artifacts and workstation capabilities, then installs the scheduler and serves HTTP; readiness is
not constructed before recovery. Shutdown drains, accounts for owned work, and only then records a
graceful runtime stop.

## Production path inventory

| Contract | Production constructors/call sites | Drift assessment |
|---|---|---|
| `ExecutionResult` | `adapters/local_workstation/execution.rs`; test mocks elsewhere | One production supervisor. It validates request/execution identity and composite cleanup before returning definite evidence. |
| `ToolTerminalOutcome` / `FinishToolExecutionRequest` | eight semantic completion/recovery helpers in `application/tool_execution_service.rs` | Multiple outcome constructors, one production service, one SQLite normalizer/validator and now one schema constraint. Independent flags can no longer silently drift into storage. |
| `ToolResultEvidence` | same service helpers for rejection, file, shell, cancellation; unknown has none | Multiple semantic constructors are necessary; shared result-class/tuple validation is canonical. |
| `ExecutionResultKind` mapping | `execution_result_class` in `tool_execution_service.rs` | Exactly one production mapping. Cleanup ambiguity is diverted to outcome_unknown before mapping. |
| Work transitions | lifecycle decisions in `domain/lifecycle.rs`, committed through SQLite work projection+journal helpers | Multiple events share the closed lifecycle validator and optimistic expected snapshot. |
| Cancellation completion | agent/runtime cancellation -> tool service -> workstation supervisor -> state store | Completion requires composite cleanup; pre-dispatch cancellation keeps N/A fields NULL. |
| outcome_unknown | `persist_outcome_unknown`, panic/uncertain workstation branches, startup recovery | One service helper for live ambiguity plus recovery logic using the same lifecycle classification; no result evidence and cleanup false. |
| startup recovery finalization | bootstrap runtime/recovery service and SQLite Stage 10 units | Unit-indexed and idempotent; readiness follows completion. |
| graceful shutdown finalization | runtime shutdown coordinator and SQLite runtime stop transition | Graceful stop is recorded only after work ownership is accounted. |
| file completion | file-read service branch | Independent from shell but writes explicit 0/0 after dispatch; no process cleanup field is applicable. |
| shell completion | `finish_run_shell` | Exactly one production constructor from non-null `ExecutionResult` flags. |
| validation/authority/spawn/timeout/signal | service semantic branches plus central validator | Multiple entry branches, shared validator and V5 storage constraint; host matrix covers process outcomes. |

## Findings inventory

| ID | Priority/type | Path | Exact assumption and real behavior | Failure mode / why missed | Minimal fix and regression | EC2? |
|---|---|---|---|---|---|---|
| S27-AUD-001 | P0 RUNTIME_DEFECT/DURABILITY | `0003`, `stage8.rs` | App normalization was expected to repair terminal flags, but schema V4 allowed old/direct completed post-dispatch NULLs and application writes do not rewrite history. | Canonical row remains ambiguous; lower-level test created a new request and never tested migration. | V5 compatible backfill + cross-column CHECK; migration repairs NULL/NULL and mixed compatible rows and rejects contradiction. | Yes, migration of the actual EBS DB. |
| S27-AUD-002 | P0 FALSE_PASS | `scripted_provider.rs`, `stage18_harness.rs`, `stage27.rs` | Global FIFO plans were assumed to belong to intended work; real agent continuation could consume the next work's answer. | Cancelled/failed tool work could complete with a follower answer. Count-only assertion missed ownership. | Redacted exact user-input hash binding plus required tool-result boundary and zero remaining-program assertion. | Linux flow yes. |
| S27-AUD-003 | P1 SECURITY_BOUNDARY | workstation launcher, unit | Clearing P/E/I/A and setting NNP was treated as clearing capabilities; Linux retains CapBnd independently. | Child did not meet the all-capability contract and retained unnecessary latent authority. | Restrict service bound to four transition caps; launcher drops and verifies every bounding capability before UID drop; child asserts all five sets zero. | Yes. |
| S27-AUD-004 | P1 FALSE_PASS/FALSE_FAIL | `production-evidence.py` | Multiple SELECTs on a read-only connection were assumed to be one snapshot; Python did not hold an explicit read transaction. | Counts, runtime, journal and recovery could describe different moments. | `query_only` plus explicit BEGIN/rollback and end-of-capture service identity recheck. | Yes. |
| S27-AUD-005 | P1 DURABILITY | `production-evidence.py` | fsyncing the temp file before rename was assumed to persist the directory entry. | Power loss could lose a supposedly frozen evidence filename. | fsync parent directory after replace; regression observes two fsync calls and create-once behavior. | Reboot comparator yes. |
| S27-AUD-006 | P1 RELEASE/TEST_IDENTITY | build/bootstrap/upgrade/evidence/verifier scripts | Clean checkout identity and a test-only source-delta exception were assumed to prove shared target output and host-test provenance. | Stale/wrong binaries could be installed under a new release name, or a newer test executable could falsely certify an older production release. | Commit-bound five-binary digest manifest, exact-entry verification, source/install digest match, installed immutable manifest and snapshot hashes/modes; require verifier source commit to equal deployment and assert the live test's embedded revision. | Yes. |
| S27-AUD-007 | P1 FALSE_PASS/FALSE_FAIL | `verify-production-host.sh` | `setpriv` identity was treated as service composition; cwd, umask and rlimits differed. | Relative paths, file modes and resource behavior could differ from production. | Set exact cwd/0077/NOFILE/NPROC and assert them inside launched Bash. | Yes. |
| S27-AUD-008 | P1 DIAGNOSTIC/FALSE_NEGATIVE | `verify-production-host.sh` | `set -e` first failure was treated as useful gate behavior for independent checks. | One host trip exposed one mismatch. | Named accumulator for safe read-only/isolated checks; residue/stateful transitions still fail closed. | Yes. |
| S27-AUD-009 | P1 FALSE_PASS | `stage27.rs` | Success+cancellation were assumed to cover terminal persistence; restart accepted several broad states before narrow fields. | Alternate nonzero/signal/timeout paths could drift; broad assertions obscured cause. | Full terminal matrix and exact restart work/tool state, flags, timestamps, cleanup and plan consumption. | Yes. |
| S27-AUD-010 | P1 SECURITY/PARITY_GAP | evidence helper and Stage 27 child | Checked-in unit text was treated as effective process composition and UID alone as identity. | Drop-ins, limits, capabilities, cwd/env, or network filter inheritance could differ. | Exact deployed asset equality; effective properties and `/proc` status/cwd/env-name evidence; child cgroup and safe IMDS denial probe. | Yes. |
| S27-AUD-011 | P1 DURABILITY/FALSE_PASS | comparator | Stable counts were compared without requiring journal/recovery advancement. | A stale/torn after-snapshot could pass identity checks. | Require monotonic journal head and new recovery event strictly after frozen head; regression mutates each. | Yes. |
| S27-AUD-012 | P1 EVIDENCE_INTEGRITY | comparator | Evidence input safety was checked only by surrounding shell in some paths. | Symlink/wrong-owner/writable snapshot could be substituted. | Helper itself requires regular non-symlink root:0600 input and create-once output. | Yes. |
| S27-AUD-013 | P1 HOST_PROOF_GAP | real service vs B/C | A CAP_SYS_ADMIN verifier child was assumed to prove the CAP_KILL-only service can create/place/clean its own execution. | Stage 27 could pass the alternate cross-delegation path while service-owned spawning failed. | Production startup now performs the combined create/setsid/pre-exec-placement/launcher/identity/capability/env/cleanup probe before readiness. | **Required.** |
| S27-AUD-014 | P2 HARNESS_PARITY | Stage18Harness | Fixture target output limits, 5s shutdown grace, temp storage and mode-relaxed workspace differ from production. | Tests outside those concerns may be mislabeled as full composition proofs. | Differences documented/classified; production config/unit exact equality and real-service checks own these assertions. | For production limits/lifecycle, yes. |
| S27-AUD-015 | P2 DIAGNOSTIC | wait helpers | Generic terminal waits and polling were assumed to prove the intended result. | A wrong terminal state had poor diagnostics. | Stage 27 callers now assert exact state/evidence/work ID; durable polling cannot miss a terminal row. | No beyond live test. |
| S27-AUD-016 | P2 ACL_TEST_SCOPE | disposable roots | Writable mode relaxation was assumed equivalent to persistent default ACLs. | Raw mode assertions could false-fail, and disposable writes could false-pass access policy. | Separate executable-path tests from exact persistent sentinel ACL/mask/owner checks. | Yes. |
| S27-AUD-017 | P1 SECURITY_BOUNDARY | `craxii-server.service`, launcher probes | UID/environment containment was assumed sufficient to keep provider material out of crash artifacts; the service allowed the host default core size. | A backend crash could hand credential-bearing memory to the core-dump subsystem. | Set and verify `LimitCORE=0`; prove the actual child inherits zero and sees neither the runtime credential path nor any descriptor above stderr. | Yes. |
| S27-AUD-018 | P1 DEPLOYMENT_RECOVERY_GAP | `upgrade-release.sh`, schema compatibility | Retaining the previous release path suggested rollback remained available. V5 is forward-only and the V4 binary rejects it after migration. | A post-migration startup failure is an outage requiring fix-forward; an automatic symlink rollback would be incorrect and could obscure the schema mismatch. | Document and fail closed. A safe automatic rollback would require a separately designed stopped-service SQLite backup/restore protocol, so it is not added inside this gate delta. | Do not induce on EC2; successful upgrade/readiness is required. |
| S27-AUD-019 | P2 HOST_TOOL_ASSUMPTION | `production-evidence.py` | ACL mode calculation unnecessarily assumed Python 3.10 `zip(strict=...)`; the repository host's system Python is 3.9 although Ubuntu 24.04 production is 3.12. | Focused evidence regressions failed before testing gate semantics, creating a local false failure. | Use ordinary `zip`; ACL parsing already proves both inputs have exactly three positions. Existing ACL regression executes on the system interpreter. | No. |
| S27-AUD-020 | P2 STRUCTURAL_CHECKER | `check-stage23-observability.mjs` | A historical Stage 23 checker assumed the durable schema must remain V4 forever; Stage 27 legitimately introduces V5. | The consolidated repository gate false-failed after all behavioral/schema tests passed. | Advance the straightforward structural expectation to the current V5 constant; behavior remains covered by migration and schema tests. | No. |

Confirmed totals: **P0 2, P1 13, P2 5**. S27-AUD-010 and S27-AUD-013 are strongly
evidenced proof gaps, not claims that the production spawn or systemd filter currently fails.
S27-AUD-018 is an explicit forward-only recovery limitation; an older binary is not presented as
a safe rollback after the migration.

## Configuration, systemd, and nullable-schema cross-checks

- The checked-in production config is now compared byte-for-byte with `/etc/craxii/config.toml`
  before the gate. It fixes Luna, 128k context, 16,384 maximum/8,192 requested output, no reasoning
  continuation, three provider attempts, 300s invocation/60s idle, 120s default/900s maximum shell,
  capture/projection limits, clean environment, fixed launcher/cgroup roots, persistent paths,
  loopback binding, and 10s shutdown grace. Stage18's default limits, 5s shutdown and temporary
  paths are explicitly test acceleration/isolation and are not used as production-config proof.
- The checked-in unit is also compared byte-for-byte and effective systemd properties are captured.
  `Delegate`, `KillMode`, restart delay, stop timeout, cwd, umask, all three resource limits,
  ambient/bounding capabilities, IP denial, fragment/drop-ins/environment and termination signal
  are checked. `LoadCredential` and `RequiresMountsFor` remain exact unit-file plus black-box
  metadata/mount checks; credential content is never read.
- Schema V4 already binds nullable provider usage/error/certainty fields by model terminal class.
  V3 binds optional tool cwd/authority/start/streams/artifacts to lifecycle state. V5 closes the one
  conflated tool pair: NULL means pre-dispatch/not-applicable or post-dispatch unknown, while every
  definite post-dispatch completion has explicit booleans matched to `result_kind`. Missing or
  unrecognized JSON kinds cannot exploit SQLite CHECK's normal NULL-success rule.
- Nullable runtime stop fields distinguish the one running row from stopped history; nullable work
  attempt IDs are state-constrained and are now explicitly counted at evidence capture. Artifact
  projection links are nullable only when captured bytes are zero/not applicable. Read-side codecs
  reject invalid integers/JSON/state tuples rather than coercing NULL to false.

## High-risk areas examined with no defect found

- Durable tool intent is committed before handler/workstation handoff. Ambiguous post-intent errors
  converge on outcome_unknown and no automatic side-effect retry.
- Linux placement occurs in `pre_exec` before user code can run. The child writes itself through an
  already-opened `cgroup.procs`; there is no post-exec fork-escape window.
- `waitid(...WNOWAIT)` retains leader identity until cleanup/reap. Cleanup combines leader reap,
  joined pipe drains, empty process group, cgroup kill/populated evidence and directory removal;
  `/proc/<pid>` disappearance alone is not accepted.
- TERM/KILL, cancellation/natural-exit/timeout/shutdown races have focused adapter tests. UUID
  execution cgroup names fail on collision and startup recovery handles stale ownership.
- SQLite production connections verify WAL, FULL synchronous, foreign keys, busy timeout and fixed
  autocheckpoint; a process lock prevents a second runtime owner. State+journal projection commits
  are transactional and startup performs integrity/foreign-key/application consistency checks.
- Published artifact without DB commit is intentionally a nonfatal reported orphan. Referenced
  objects are content/hash/length verified before readiness.
- Startup constructs no serving HTTP state and no scheduler until recovery and workstation
  validation finish. Liveness/readiness are separately validated.
- Launcher already cleared supplementary groups, P/E/I/A capabilities, environment and unrelated
  descriptors, used fixed Bash/reader paths, and set NNP. The audit added the missing bound-set step.
- No AWS SDK/credential-discovery dependency or AWS command path exists. Credential content/hash is
  not inspected by Stage 27 evidence.
- Core dumps are disabled for the credential-bearing service and descendants. The launcher closes
  every descriptor above stderr, and the service-owned startup probe now rejects a readable runtime
  credential or inherited descriptor before readiness.
- Persistent layout scripts pin UUID, ext4 FSROOT and exact bind roots; service `RequiresMountsFor`
  and post-reboot mount verification prevent an empty-root fallback.
- Timestamps are UTC canonical wall-clock evidence while deadlines/polling use monotonic time;
  bounded integer conversions fail instead of wrapping. Shell output drains run concurrently, so
  capture-limit overflow, binary/invalid UTF-8 projection, and full pipes do not deadlock the child.

## Remaining minimum real-host proof

The next gate must validate the new release/migration; exact active systemd properties and main
process identity/capabilities/cwd/environment names; launcher child UID/GID/groups/all capability
sets/NNP/session/cwd/umask/rlimits/environment; inherited IMDS denial; success/nonzero/signal/
timeout/cancellation evidence; descendant cleanup/no late effect; crash/outcome_unknown/no
redispatch; actual service restart with control-process survival; recovery-before-ready; SQLite
integrity/runtime singularity/journal ordering; release/MainPID identity; ACL-effective access;
mount UUID/FSROOT/binds; clean cgroup root; and a create-once internally consistent pre-reboot
snapshot. Production startup's CAP_KILL-only substrate probe must pass separately from the
verifier's CAP_SYS_ADMIN path; a prior provider benchmark is not to be rerun.
The forward-only upgrade failure/restore path is not destructively exercised by Stage 27; if the
new service cannot become ready, stop and fix forward rather than starting the V4 release on V5.
