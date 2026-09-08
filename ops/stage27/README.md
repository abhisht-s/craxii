# Stage 27 Linux security boundary

These assets prepare the local Linux trust split for a later, explicitly authorized host run. They
do not call cloud APIs, configure networking/TLS, mount or format storage, install credentials, or
start the service.

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
starts.

`bootstrap-security-boundary.sh` requires an already-built release directory and an already-mounted
filesystem layout. It installs users, permissions, binaries, the unit, and the non-secret config
template, but deliberately leaves the unit stopped and does not create a provider credential.

`verify-security-boundary.sh` is a pre-credential host check. It uses synthetic canaries only and
refuses to run if the provider credential path already exists.

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
