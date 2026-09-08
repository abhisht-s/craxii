# Stage 27 Linux security boundary

These assets prepare the complete precredential Linux host boundary for an explicitly authorized
browser Session Manager run. They do not call cloud APIs, configure networking/TLS, start the
service, or install a provider credential. `bootstrap-data-volume.sh` is the only asset that can
format storage, and it does so only after the explicit confirmation argument and its fail-closed
80 GiB whole-disk checks pass.

The production contract is:

- trusted backend: `craxii-server:craxii-server`;
- model-controlled workstation: `craxii:craxii`;
- model execution: fixed root-owned `craxii-workstation-launcher`, executable only by
  `craxii-server`, with all supplementary groups and capabilities dropped before the fixed
  operation is executed;
- production administrative execution: disabled;
- workspace sharing: a narrow ACL for `craxii-server`; and
- backend state, config, credentials, and releases: inaccessible for write/read as appropriate to
  `craxii`.

The trusted service retains only `CAP_KILL` so its existing TERM/KILL cancellation sequence can
cross the workstation UID boundary. The launcher clears all capabilities before the model command
starts. The systemd unit denies both EC2 Instance Metadata Service addresses for the service cgroup
and all delegated descendants, so the host's SSM instance role is not reachable by model work.

`install-host-prerequisites.sh` converts only official Ubuntu archive/security APT URLs from HTTP
to HTTPS, installs the minimal package set, creates a locked `craxii-build` account, and installs
Rust 1.98.0 under `/var/lib/craxii-build`.

`bootstrap-data-volume.sh` refuses caller-selected device names. It requires exactly one
unformatted, unmounted, whole 80 GiB disk with no partitions, holders, swap/mount use, filesystem,
or other device signature. It formats that sole candidate as ext4, records its UUID in
`/etc/fstab`, creates the three persistent bind mounts, and uses a UUID marker for idempotent
verification. An initialized but unrecognized disk is never reformatted.

`build-release.sh` clones the public repository as the locked build user, requires the requested
40-character commit to be reachable from `origin/main`, checks out that exact commit, verifies a
clean tree, and builds the four locked release binaries with `CARGO_BUILD_JOBS=2`.

`bootstrap-security-boundary.sh` requires an already-built release and verified data layout. It
installs users, permissions, binaries, the unit, and the non-secret production config, but leaves
the unit stopped and disabled and does not create a provider credential.

`verify-precredential-host.sh` is the real-host precredential check. It verifies Ubuntu/CPU/cgroup,
UUID/fstab/bind mounts, the controlled toolchain and exact source revision, users/modes/ACLs,
launcher identity/environment/FD/capability behavior, systemd/config, and absence of a real OpenAI
credential. It uses synthetic canaries only and makes no provider or AWS request.

`install-provider-credential.sh` is deliberately separate. After the real-host verifier passes, a
human may run it in the browser terminal. It reads without echo, does not use an argument or
environment variable, refuses overwrite, and installs the systemd credential source as
`craxii-server:craxii-server` mode `0600`. Running that script is outside the precredential run.

The older `scripts/verify-stage13-ubuntu-target` suite remains a separate privileged host-capability
test. It now requires `CRAXII_STAGE13_CREDENTIAL_FREE_DISPOSABLE=1` and refuses the production
`craxii-server.service` unit.

## User-switch decision

Problem: a credential-bearing backend cannot execute model-controlled work under its own UID, and
the non-root backend cannot directly call `setresuid` for another user.

Considered approaches were a broad `sudo -u` policy, a PAM/session wrapper, granting the service
general privilege, and a fixed setuid launcher. The broad mechanisms expose more authority and
policy surface than this stage needs. The decision is one root-owned setuid launcher with only
`shell` and `read-file` operations, a fixed caller (`craxii-server`), and a fixed target (`craxii`).
It clears groups, capabilities, ambient authority, environment, and inherited file descriptors
before the model-controlled executable begins. The adjacent reader opens files only after the same
drop.

The trusted backend retains `CAP_KILL` solely to preserve graceful TERM/KILL process supervision
across the UID boundary; the launcher proves that capability does not reach its child. Root remains
trusted host administration, so this design does not claim protection against root compromise.

Rollback is to stop the service and restore the previous release/config assets. A credential must
not be present while rolling back to a same-UID or administrative-execution design.
