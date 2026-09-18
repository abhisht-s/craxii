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

The trusted server and offline-admin binaries are `root:craxii-server` mode `0550`. The fixed
reader is `root:root` mode `0111`: it remains executable after the launcher drops identity, but its
image is not readable by model-controlled work. The setuid launcher remains
`root:craxii-server` mode `4750` and rejects every caller except the real `craxii-server` UID.

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
clean tree, and builds the five locked release binaries with `CARGO_BUILD_JOBS=2`. The fifth is the
one-shot `craxii-stage27-luna-benchmark` operator binary. It validates the active release and live
runtime, reads the retained device bearer only from a hidden `/dev/tty` prompt, submits the exact
canonical prompt once, and verifies the resulting Luna/model/tool/host evidence without reading the
provider credential or retaining raw model content.
The build also writes a commit-bound SHA-256 manifest; bootstrap and upgrade refuse binaries that
do not match it, preventing a clean checkout from lending identity to stale target output.

`bootstrap-security-boundary.sh` requires an already-built release and verified data layout. It
installs users, permissions, binaries, the unit, and the non-secret production config, but leaves
the unit stopped and disabled and does not create either provider credential. The checked-in
configuration is an explicit Telegram hard-off default:

    [credentials]
    source = "systemd"
    declared = ["openai_provider"]

    [telegram]
    enabled = false

`render-config.py` renders this template before installation and the candidate `craxii-admin`
validates the result with the normal Rust configuration contract. On upgrade, the renderer takes
all non-Telegram settings from the new template while preserving the installed Telegram enabled
state, ChannelAccountId, expected bot ID, and owner ID. An older config with no `[telegram]` table
is treated as disabled. The renderer canonicalizes only these nonsecret fields; it never accepts a
token. Deployed-asset verification reconstructs the expected config with the same preservation
rule instead of requiring the host-owned config to remain byte-identical to the repository
template.

`upgrade-release.sh` is the post-provisioning immutable-release path. It requires an exact clean
build checkout, then performs a metadata-only preflight of the mandatory
`/etc/craxii/credentials/telegram_bot` source before it creates staging or release state, stops the
service, replaces config/unit assets, or changes the active release pointer. The preflight requires
the same directory ownership/mode and regular, single-link, nonempty credential ownership/mode as
the installer and never opens the credential. Only after that gate does the upgrade verify the
matching build manifest, install a new five-binary release plus the audited non-secret config/unit
(never the credential), stage and validate the mutable assets before stopping the healthy service,
or require a failed/inactive recovery service to have no live MainPID. It then atomically replaces
each installed file, reloads systemd, and proves the manager has no pending reload before atomically
advancing `/opt/craxii/current`. It clears any exhausted start-rate counter, performs one candidate
start, and requires readiness and exact MainPID release identity before success.
Once candidate startup has been attempted, any later deployment failure stops the unverified
candidate instead of leaving it active or restart-looping.
Schema V5 is forward-only: once the new binary applies it, the V4 release is not a valid rollback
target. The upgrade therefore fails closed and requires fix-forward if post-migration readiness
does not succeed; it never restores an older binary over a newer database.

`verify-precredential-host.sh` is the real-host precredential check. It verifies Ubuntu/CPU/cgroup,
UUID/fstab/bind mounts, the controlled toolchain and exact source revision, users/modes/ACLs,
launcher identity/environment/FD/capability behavior, systemd/config, and absence of a real OpenAI
credential. It uses synthetic canaries only and makes no provider or AWS request.

`install-provider-credential.sh` is deliberately separate. After the real-host verifier passes, a
human may run it in the browser terminal. It reads without echo, does not use an argument or
environment variable, refuses overwrite, and installs the systemd credential source as
`craxii-server:craxii-server` mode `0600`. Running that script is outside the precredential run.

## Telegram production configuration and credential

The enabled configuration has exactly this additional contract; the three identity values are
nonsecret and host-specific:

    [credentials]
    source = "systemd"
    declared = ["openai_provider", "telegram_bot"]

    [telegram]
    enabled = true
    channel_account_id = "<canonical UUIDv7>"
    credential = "telegram_bot"
    expected_bot_user_id = 10001
    owner_telegram_user_id = 20002

Generate the ChannelAccountId once with the repository-native UUIDv7 type, then preserve it for
the lifetime of this Telegram channel account:

    sudo /opt/craxii/current/craxii-admin \
      --config /etc/craxii/config.toml channel-account-id generate

Do not regenerate it during an upgrade. Record the printed UUIDv7 for the config-rendering step.
The command reads neither credential and prints only the canonical ID to stdout.

The fixed production service unit maps OpenAI and Telegram separately with `LoadCredential` and
always declares both mappings. Systemd therefore requires both source files whenever that unit
starts, even while application-level Telegram is disabled. The token is still not requested or
read by the application while Telegram is disabled. First bootstrap remains safe before either
credential is installed because bootstrap leaves the service stopped and disabled.

For a legacy Telegram-disabled production upgrade, the supported order is:

1. Keep the existing incumbent service running and healthy.
2. Install the Telegram credential securely with the create-once installer. Creating this new,
   unused source file does not disturb the legacy incumbent.
3. Verify its metadata without reading its contents.
4. Run the release upgrade. The preflight refuses to create deployment state or touch the
   incumbent if the credential is absent or unsafe.
5. Let the candidate start with Telegram disabled when the preserved config is disabled or has no
   legacy `[telegram]` table.
6. Later, when live Telegram is explicitly authorized, stop the service, render and validate the
   enabled config, atomically install it, and restart the service.

Steps 2 and 3 are:

    sudo /usr/bin/python3 \
      /var/lib/craxii-build/source/ops/stage27/install-telegram-credential.py --install
    sudo /usr/bin/python3 \
      /var/lib/craxii-build/source/ops/stage27/install-telegram-credential.py --verify

The prompt is hidden. The token is never accepted in argv or an environment variable, and is
installed only at `/etc/craxii/credentials/telegram_bot` as `craxii-server:craxii-server` mode
`0600`. The installer refuses an existing file and never reads or overwrites `openai_provider`.
Create-once installation is safe while the legacy incumbent is healthy because that process does
not know or consume the new source file; explicit rotation still requires a stopped service.

When later enabling live Telegram, stop the service and render the enabled config using the
generated UUIDv7 and the real nonsecret numeric IDs. Use a new same-filesystem pending path so the
final rename is atomic:

    sudo systemctl stop craxii-server.service
    pending=/etc/craxii/.config.toml.telegram.$$
    sudo /usr/bin/python3 \
      /var/lib/craxii-build/source/ops/stage27/render-config.py \
      --template /etc/craxii/config.toml \
      --enable-telegram \
      --channel-account-id '<generated-once-uuidv7>' \
      --expected-bot-user-id '<bot-numeric-id>' \
      --owner-telegram-user-id '<owner-numeric-id>' \
      --output "$pending"
    sudo chown root:craxii-server "$pending"
    sudo chmod 0640 "$pending"
    sudo /opt/craxii/current/craxii-admin --config "$pending" config validate
    sudo mv -Tf "$pending" /etc/craxii/config.toml
    sudo systemctl start craxii-server.service
    curl --fail --silent --show-error http://127.0.0.1:8080/health/ready >/dev/null

To return to hard-off mode, repeat the stopped-service render/validate/atomic-rename flow with
`--disable-telegram` and no Telegram identity arguments. The resulting config declares only
`openai_provider` and contains only `enabled = false` under `[telegram]`. Keep the Telegram source
file in place because the fixed unit mapping is mandatory; the Rust application does not load it
while disabled.

Rotation is a separate explicit operation. Stop the service first, rotate through another hidden
prompt, then start it and require loopback readiness:

    sudo systemctl stop craxii-server.service
    sudo /usr/bin/python3 \
      /var/lib/craxii-build/source/ops/stage27/install-telegram-credential.py --rotate
    sudo systemctl start craxii-server.service
    curl --fail --silent --show-error http://127.0.0.1:8080/health/ready >/dev/null

Rotation writes and fsyncs a restrictive same-directory temporary file, atomically renames it over
the old credential, and fsyncs the directory. A failure before replacement leaves the prior token
intact; a failure leaves the service stopped for explicit operator recovery. There is deliberately
no local Telegram call or token validation.

## Minimum local operations

All state-sensitive admin commands are offline operations. They acquire the same exclusive Craxii
state lock as the server and fail closed if the server owns it. Use the fixed lifecycle and
loopback readiness commands:

    sudo systemctl start craxii-server.service
    curl --fail --silent --show-error http://127.0.0.1:8080/health/ready >/dev/null

    sudo systemctl stop craxii-server.service
    systemctl show craxii-server.service --property=ActiveState --property=MainPID

    sudo systemctl restart craxii-server.service
    curl --fail --silent --show-error http://127.0.0.1:8080/health/ready >/dev/null

Before an offline operation, the shown service state must be inactive or failed and MainPID must
be 0. A readiness failure is not permission to continue with state mutation.

To inspect only the operational outbound states, stop the service and run:

    sudo /opt/craxii/current/craxii-admin \
      --config /etc/craxii/config.toml delivery inspect \
      --state queued --state retry_wait --state permanent_failure --state outcome_unknown \
      --limit 100

The output is bounded and contains internal IDs, state, part/attempt counts, retry/deadline timing,
normalized failure class, and timestamps only. It never prints payload text, external Telegram
IDs, provider descriptions, credentials, model context, or artifacts. Repeat with one --state
for a narrower view; use the last internal delivery ID with --after for the next bounded page.

Disabling a Telegram channel account is a durable, one-way local admin transition. First stop the
service and install a validated Telegram hard-off config using the stopped-service
render-config.py --disable-telegram flow above. Then use the stable internal
ChannelAccountId—never a Telegram bot, chat, or user ID:

    sudo /opt/craxii/current/craxii-admin \
      --config /etc/craxii/config.toml channel-account disable \
      '<stable-channel-account-uuidv7>'

The only successful outcomes are disabled or deterministic already_disabled. There is no admin
re-enable or delete command. Keep Telegram hard-off before starting the service again; enabled
Telegram startup intentionally refuses the disabled account rather than reviving it.

### Pre-deployment recovery copy

Take a verified recovery copy before every deployment that may migrate SQLite, including V5→V7
and V6→V7. Stop the service, verify MainPID=0, create a private destination directory, and choose
new destination names. The helper rechecks service state, takes the Craxii exclusive lock, uses
SQLite's backup API to include committed WAL state, and refuses overwrite:

    sudo systemctl stop craxii-server.service
    systemctl show craxii-server.service --property=ActiveState --property=MainPID
    sudo install -d -o root -g root -m 0700 /var/lib/craxii/recovery
    sudo /usr/bin/python3 \
      /var/lib/craxii-build/source/ops/stage27/recovery-copy.py create \
      --source-state-root /var/lib/craxii \
      --destination-db /var/lib/craxii/recovery/<new-name>.sqlite3 \
      --manifest /var/lib/craxii/recovery/<new-name>.manifest.json \
      --repository-sha '<candidate-40-character-commit>'

Never use a raw main-database file copy as a recovery method. The helper does not read or copy
credentials, artifacts, workspace contents, or SQLite sidecars into the recovery bundle. It
creates only a mode-0600 self-contained database and mode-0600 redacted manifest. Validate the
pair independently before deployment:

    sudo /usr/bin/python3 \
      /var/lib/craxii-build/source/ops/stage27/recovery-copy.py validate \
      --database /var/lib/craxii/recovery/<new-name>.sqlite3 \
      --manifest /var/lib/craxii/recovery/<new-name>.manifest.json

Validation requires both SQLite quick_check and full integrity_check, an empty foreign_key_check,
exact contiguous SQLx migration descriptions/checksums, a supported schema version and ceiling,
the manifest digest/size, no WAL dependency, and safe file permissions. Source truth is: a V5 candidate startup applies 0006 and 0007; V6 applies 0007; V7 applies none. The verified recovery
copy is still mandatory before any of those deployment/startup actions.

### Inactive-replacement restore boundary

A CH-6 recovery copy is an inactive recovery point, not a second active authority or complete
host backup. Never restore over /var/lib/craxii/db/craxii.sqlite3 or any database used by an
active service. With the original service stopped, validate the selected recovery pair first,
create a new private replacement state root, install the recovery database there, then validate
the installed copy again before any candidate config points at it:

    sudo install -d -o root -g root -m 0700 \
      /var/lib/craxii-replacement /var/lib/craxii-replacement/db \
      /var/lib/craxii-replacement/locks
    sudo install -o root -g root -m 0600 \
      /var/lib/craxii/recovery/<verified-name>.sqlite3 \
      /var/lib/craxii-replacement/db/craxii.sqlite3
    sudo install -o root -g root -m 0600 /dev/null \
      /var/lib/craxii-replacement/locks/craxii.lock
    sudo /usr/bin/python3 \
      /var/lib/craxii-build/source/ops/stage27/recovery-copy.py validate \
      --database /var/lib/craxii-replacement/db/craxii.sqlite3 \
      --manifest /var/lib/craxii/recovery/<verified-name>.manifest.json

Do not point a candidate at the replacement until that final validation passes. Never run a binary
whose schema ceiling is older than the restored database. A full active-host restore rehearsal,
artifact/workspace restoration, provider activation, and replacement-host automation remain
outside CH-6.

### Forward-only migration failure

If startup reaches V6 and 0007 then fails, if a migration succeeds but the candidate later fails,
or if the available rollback binary is older than the database schema:

1. Stop the candidate and require MainPID=0.
2. Preserve the failed state root and sanitized startup/migration evidence; do not modify it to
   imitate an older schema.
3. Do not start the older binary against that database and do not perform an in-place rollback.
4. Choose either a compatible fix-forward candidate, or a new inactive replacement state root
   populated from the verified pre-migration recovery copy and validated as above.
5. Point exactly one compatible candidate at the chosen inactive state only after validation, then
   start once and require loopback readiness.

The same rule applies when V5→V6 succeeds but V6→V7 fails: the failed V6 state is evidence, not a
valid target for the older V5 binary.

Never store the Telegram token in TOML, `Environment=`, `EnvironmentFile=`, a shell argument, a
repository file, or an operator log. Telegram uses outbound long polling, so enabling it does not
change the loopback bind and requires no public listener, webhook, or Caddy route.

`verify-production-host.sh` owns the final production-like Stage 27 restart/reboot gate. Its
`--pre-reboot` mode creates two fixed non-secret persistence sentinels, captures a read-only
canonical-state snapshot, runs focused recovery/ambiguity checks with a scripted provider, runs a
real Linux terminal-outcome matrix and cancellation through the installed privilege-drop launcher
and delegated execution cgroup, and coordinates one normal systemd service restart while both an
owned execution and an outside control process are alive. Every Rust live-test worker enters the
delegated execution root before dropping to `craxii-server` and retains only the production
`CAP_KILL` effective authority and transition bounding set; no cross-delegation `CAP_SYS_ADMIN`
path is used. It then writes a redacted JSON evidence bundle and restart
comparison under `/srv/craxii-data/stage27-evidence`. It never reads or hashes the provider
credential; the credential check is limited to file metadata and a negative model-child access
probe. Independent read-only and isolated checks accumulate a consolidated result; cgroup residue
and stateful transition failures still stop immediately. It never invokes a real provider.
Snapshots accept only the exact validation attestations for their named phase, run both SQLite
quick and full integrity checks, and retain deterministic aggregate fingerprints for every stable
canonical table (excluding only the workstation observation timestamp refreshed at startup), plus
a rolling journal-prefix commitment and exact stream-head/sequence consistency result.
They also revalidate the live persistent-directory ownership, the workspace access/default ACLs,
the locked and separate backend/workstation identities, and single-link private database/artifact
metadata so an old bootstrap result cannot substitute for the current security boundary.
The restart comparator binds the immediate durable runtime predecessor and the effective systemd
start incarnation; it deliberately does not treat a recyclable diagnostic PID as identity.
Evidence publication is atomic, create-once, and bounded on read.

The same script's `--post-reboot` mode is intentionally run only after the human reboot boundary.
It verifies automatic systemd startup, a changed Linux boot/runtime identity, graceful closure of
the pre-reboot runtime, unchanged canonical identity/state/sentinels/artifacts/release, restored
mount topology, recovery evidence before readiness, an empty execution cgroup, loopback-only
operation, and the unchanged credential access boundary. Evidence files are create-once and are
never overwritten.

Run the production gate from the controlled source checkout with the deployed release commit and
data-filesystem UUID as explicit arguments:

```sh
sudo /bin/bash /var/lib/craxii-build/source/ops/stage27/verify-production-host.sh \
  --pre-reboot <deployed-40-character-commit> <data-filesystem-uuid>
```

After a human reboots the instance through the AWS Console and Session Manager becomes available:

```sh
sudo /bin/bash /var/lib/craxii-build/source/ops/stage27/verify-production-host.sh \
  --post-reboot <deployed-40-character-commit> <data-filesystem-uuid>
```

Neither mode calls AWS APIs, accepts a bearer/provider secret, changes the immutable release, or
performs the EC2 reboot.

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
