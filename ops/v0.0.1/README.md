# Persistent guest boundary assets

These assets are an unpublished implementation candidate awaiting separate independent verification.
Credential-free installed Linux qualification complements source tests; it grants no live activation
or provider authority. Real infrastructure and image qualification remain separately gated.

The standalone guest file contract is version 1. It is independent of the frozen worker protocol.
The exact schema and small corpus are mirrored in the control plane with SHA256 pins. Config bytes
are limited to 16 KiB, 4096 nodes and depth 16. Unknown fields, duplicate decoded keys, invalid UTF8,
noncanonical integers/identities/CIDRs and mismatched material digests refuse. Byte quantities are
canonical decimal strings. Production configuration is unsupported. Synthetic provenance supplies
only test data and cannot select an installed bootstrap backend or authorize a live resource.
A future trusted infrastructure composer owns physical provider observations; a digest/serial or
worker signature by itself proves neither encryption nor attachment authority.

The installed root initializer has no device argument, environment override or fixture selector.
It resolves one exact EBS serial/model match, verifies 80 GiB, rejects root ancestry/partitions,
holders/swap/foreign mounts/UUID clones and reads the whole blank disk before the first format.
Only `initialize_blank` permits that format. A root-only flock serializes mutations. Atomic receipt
publication fsyncs the file and parent before `mke2fs` runs. The block descriptor stays pinned while
mke2fs obtains its own exclusive claim. Every recorded `format_intended` is inspection-only on
restart, even if the disk still appears blank. Unknown or contradictory effects hold; no repair
reformats, regenerates keys, deletes customer files or resets a journal.

`adopt_existing` and `adopt_restored` never format. Their immutable birth marker remains historical;
a replacement installation is checked separately. Restored adoption additionally requires explicit
source snapshot/volume/lineage material. An unmounted adoption is inspected read-only with noload
before enabling writes. Missing marker/layout/journal evidence holds. Unknown legacy disks are not
adopted. The actual guest entrypoints must verify provider facts again during later qualification.

Fixed ext4 layout: `/srv/craxii-data/state` binds to `/var/lib/craxii`, `workspaces` to
`/srv/craxii/workspaces`, and `home/craxii` to `/home/craxii`. Required mounts are rw,nosuid,nodev.
The OS-root identity directory is `/var/lib/craxii-worker/identity`, separate from snapshot data.
Empty fallback targets are root-owned and service/model unwritable. The root lineage marker is
0440/root:worker; trusted state is worker0700/files0600; customer home/workspace belong to craxii.
The workspace ACL gives the worker observation access. There is no recursive adoption chown.
Only exact owned fstab entries are reconciled; noauto and bootstrap ordering prevent UUID-only boot
selection before physical identity checks.

The root installer leaves services stopped/unenabled. Distribution runs as craxii-server with
CAP_KILL; the fixed setuid launcher transitions to craxii, clears groups/caps/FDs/env, sets child NNP
and bounds descriptors/core dumps. Parent NNP remains disabled so the launcher can operate. Installed
LocalWorkstation composition has no administrative/direct-execution profile. Historical native
composition remains separate. Root pre-start verifies UID/proc/core/persistence/network/quota facts
and writes a boot/config-bound protected receipt. Rust rechecks mounts/paths/keys/controllers before
canonical writes and new admissions. A one-shot journal creation sentinel holds after an interrupted
first open instead of creating a second empty journal. No model/tool runner is added to distribution.

Profile 1 sets aggregate untrusted pids64, memory256MiB, swap0, CPU1core; per-execution controllers
must exist and enforce before spawn. Existing command/output/time/grace bounds remain. NOFILE1024,
CORE0, model ext4 block quota64GiB/inodes1000000, scratch tmpfs256MiB each/inodes65536, and trusted
free reserves4GiB/65536 inodes are explicit protective settings, not entitlements. Missing controllers,
quota formats or guards remain unavailable. The image must include the kernel's quota modules.
UID-level nft IPv4/IPv6 metadata/private-network denial also applies outside execution cgroups;
model loopback is denied to close protected-proxy paths. No protected local guest service is exposed.
The model UID997 cannot reach FIB routes of type local, broadcast or multicast in either family,
including guest primary/public addresses, IPv4-mapped aliases, limited/directed broadcasts and
multicast loop delivery to wildcard listeners. Every lo output route and IPv4/IPv6 link-local
service range (169.254/16 and fe80::/10) is also denied. Multicast is explicitly unavailable to tools:
any group can have a privileged local subscriber. Approved non-local unicast TCP/UDP still works.
The inet OUTPUT filter runs at priority -150 before local loop delivery reaches INPUT. Active kernel
expressions, table activation, family, chain identity, hook, priority and order must match exactly.
Each guest-shell and startup IPC probe requests a fixed root-owned systemd oneshot to recheck the
active policy, with a finite timeout, before privilege drop or any untrusted instruction. This gives
neither worker nor launcher CAP_NET_ADMIN. A failed/missing check refuses execution. Root reload
verifies disk and active policy, atomically replaces only the owned table, and verifies again;
missing/conflicting enforcement holds. Only trusted root may administer this network namespace.
Root-only system policy denies model cron/at/linger/service/privilege helpers and root scratch.

Installed `guest-shell` additionally requires a compiled seccomp filter after the UID/capability
drop and Landlock setup, before bash. Only AF_INET/AF_INET6 socket creation is allowed; socketpair
has the same family restriction. Unix pathname and abstract sockets, netlink and vsock are denied
with EPERM, covering system logging and privileged local control services. io_uring_setup and
pidfd_getfd are denied to prevent alternate socket/descriptor acquisition. Compat ABIs are rejected;
after installation, seccomp and prctl(PR_SET_SECCOMP) are denied as well, preventing untrusted
RET_LOG/LOG-flag filter installation from feeding kernel audit logging.
this installed policy supports x86_64 only. No optional policy file or environment override exists.
Every descendant inherits the filter across fork/exec; filter installation failure refuses execution.
The unit runs `guest-check-ipc` through the same launcher/drop/filter before worker startup.

Tool stdin must be /dev/null and stdout/stderr must be capture pipes. Higher inherited descriptors
are closed on exec, including activation sockets and io_uring rings. Trusted worker journal
diagnostics remain outside the tool filter. Tools must use bounded stdout/stderr for diagnostics;
direct syslog/journald and local Unix socket clients (including SSH agents, Docker, local database
sockets and socketpair-based workloads) are unavailable in this installed profile. Use approved
TCP/UDP networking under the existing UID firewall. This adds no administrative tool capability.
The boundary assumes trusted launcher/worker/kernel and root-owned installation; kernel/root
compromise and a privileged service deliberately exposed on an approved IP endpoint remain outside
this policy. Image qualification must keep privileged logging/control services on non-local network
peers unavailable to the model UID as part of the approved network configuration.

Installed shells require Landlock ABI >=3 and receive one UUID directory under each bounded tmpfs:
`/tmp/craxii-tools`, `/var/tmp/craxii-tools`, `/dev/shm/craxii-tools`. Parents are root:worker1773,
execution directories model0700. TMPDIR/TMP/TEMP name the private /tmp directory. Landlock permits
persistent workspace/home writes and only that execution's scratch; shared temp-root writes and
other executions' temp access are denied, including path-resolution aliases. Processes inherit the
policy. Scratch is ephemeral and cannot spill onto the root or persistent filesystem. Commands that
hardcode shared temp files may receive a permission denial; they must use their assigned temp path.
Each tmpfs has a shared 256MiB/65536-inode ceiling, not a per-task allocation or a persistent entitlement.
Cleanup requires an empty execution cgroup and is bounded by the caller's remaining deadline. Missing
proof, an exhausted deadline, or hostile directory permissions produce OutcomeUnknown/cleanup pending,
block dispatch and prevent idle claims. Root pre-start refuses any surviving model UID process before
removing only canonical scratch UUID directories, using descriptor-relative, same-device, non-following
bounded traversal. It never cleans persistent customer files. OS-root cloud-init/provider-state paths
and logs are protected as part of the credential boundary; trusted OS logging ownership is preserved.
Root configure recreates the protected /run directory and reapplies volatile IPC permissions on
every boot before the worker pre-start receipt; image-time directory creation alone is insufficient.


Local observations combine registry cleanup, all model-UID processes, cgroup population, durable
receipt/export/uncertainty counts, passive journal checkpoint and current storage health. Missing
visibility is unknown; pending work is busy. These observations supply no drain token, idle timer,
cloud stop, compute authorization or external-effect rollback.

The focused VM runner requires root-owned disposable fixture markers and a matching QEMU SMBIOS
identity. It uses owned sparse images/recorded loop handles only, actual ext4/binds/UID quotas,
private delegated cgroups and a fresh network namespace containing fake metadata addresses.
Its Python SQLite probe is synthetic storage evidence, not Rust SQLite schema qualification.
Kernel probes complement the small parser/phase tests. `backend/tests/guest_installed.rs` is a separate
ignored test harness requiring the exact owned x86 guest and actual shipped systemd pre-start/identity/
delegation policy. It exercises standard Rust runtime and installed launcher composition with synthetic
keys/endpoints, cgroup resource events, scratch, hostile shell, cancellation and integrated observations.
A root fixture controller separately exercises worker interruption and injected observation faults.
`v001_guest_recovery.py` and `v001_guest_storage_edges.py` use only recorded disposable images/loops;
small loop serial/size projections are explicit test injections, never installed CLI configuration.
No local test claims provider encryption or physical EC2 identity proof. No host device, credential/home or Docker socket is
mounted. VM artifacts/evidence stay outside Git. Real AMI/Nitro/encryption and EC2 reboot certification
remain separate. No service activation, infrastructure adapter, migration or worker-wire change is
introduced by these assets.

The Stage 14 candidate adds a separate root compute watchdog before storage bootstrap and
worker startup. Its protected policy is `/etc/craxii/compute-watchdog.json`; signed bootstrap
funding and the consumed receipt live under `/var/lib/craxii-compute` on the OS root disk.
The worker stages centrally signed authorization in its protected `compute-inbox`; it cannot
mint grants or write root receipts. CLOCK_BOOTTIME and the saved absolute cutoff survive
process restart. A fresh boot requires fresh signed power authority. Bootstrap permits never
authorize tools. Missing authority fails closed through the fixed local poweroff path.

The installer supplies binaries, directories and units without activation or funding. The
worker remains receipt/export-only. The separate bounded-tool qualification profile requires
real compute funding and both debug assertions and `test-failpoints`. Local installed
qualification exercises signed tool admission, active cancellation, descendant cleanup,
durable export recovery and independent guest poweroff. Separate independent verification
remains required. These candidate assets grant no workstation activation or provider authority.
Provider poweroff/billing behavior remains unqualified.

Protected compute receipts use format 2. They retain the greatest boot-wide UTC minus
CLOCK_BOOTTIME offset across bootstrap, signed renewals and process restarts. A new
deadline is bounded by the signed effective horizon (the earlier of `expires_at` and
`issued_at + max_elapsed_ms`) projected through that retained offset. Forward observations
can tighten this bound; rollback cannot extend it or replenish the cumulative drift budget.
A later signed response may reuse existing funding without extending its cutoff.
Boot tick counters are decimal strings in the receipt, independent of frozen wire formats.

Format 1 receipts lack reliable projection history and cannot resume execution on the same
boot. They remain cryptographically checked consumed-permit history for a subsequent boot
with a distinct freshly funded power operation and permit. Never delete a legacy receipt or
its consumed marker to obtain a new timer. Missing or corrupt evidence fails closed. Install
the matching worker and watchdog together; an older consumer must not resume a format 2
receipt. These changes do not alter the signed grant, policy or bootstrap-permit contracts.


The debug `test-failpoints` worker distribution fixture has an explicit `proxy` action for
signed `proxy_loop_v1` qualification. Its trusted service input supplies `model_proxy`
contract 1 pins (model ID, model/price UUIDs and digests, maximum output tokens), with no
provider endpoint, headers or credentials. The existing root/service-protected input,
installed worker identity, trusted SQLite, signed compute grant and guarded work claim
are required. The preserved AgentLoop runs with only the signed `run_shell` tool set and
the original five-second tool bound. Model waits poll existing durable controls and honor
the independent root cutoff; they cannot renew funding. Ordinary distribution startup
remains receipt/export-only and readiness unavailable. This fixture is not a production
enablement switch or a live model qualification.
