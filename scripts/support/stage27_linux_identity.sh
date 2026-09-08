#!/usr/bin/env bash
set -euo pipefail

[[ "$(uname -s)" == "Linux" ]]
[[ "$(id -u)" -eq 0 ]]

cargo build --locked -p craxii-server \
  --bin craxii-server \
  --bin craxii-admin \
  --bin craxii-workstation-launcher \
  --bin craxii-workstation-reader

groupadd --system craxii-server
groupadd --system craxii
useradd --system --gid craxii-server --home-dir /var/lib/craxii \
  --no-create-home --shell /usr/sbin/nologin craxii-server
useradd --gid craxii --create-home --home-dir /home/craxii --shell /bin/bash craxii

install -d -o root -g root -m 0755 /opt/craxii /opt/craxii/releases /opt/craxii/releases/test
install -o root -g craxii-server -m 0550 /craxii-target/debug/craxii-server /opt/craxii/releases/test/craxii-server
install -o root -g craxii-server -m 0550 /craxii-target/debug/craxii-admin /opt/craxii/releases/test/craxii-admin
install -o root -g root -m 0111 /craxii-target/debug/craxii-workstation-reader /opt/craxii/releases/test/craxii-workstation-reader
install -o root -g craxii-server -m 4750 /craxii-target/debug/craxii-workstation-launcher /opt/craxii/releases/test/craxii-workstation-launcher
ln -s releases/test /opt/craxii/current

install -d -o root -g craxii-server -m 0750 /etc/craxii
install -d -o root -g root -m 0700 /etc/craxii/credentials
install -d -o craxii-server -g craxii-server -m 0700 /var/lib/craxii
install -d -o root -g root -m 0755 /srv/craxii /srv/craxii/workspaces
install -d -o craxii -g craxii-server -m 0770 /srv/craxii/workspaces/primary

credential=/etc/craxii/credentials/synthetic-provider
config=/etc/craxii/synthetic-config
state=/var/lib/craxii/synthetic-state
workspace=/srv/craxii/workspaces/primary/synthetic-workspace
launcher=/opt/craxii/current/craxii-workstation-launcher
reader=/opt/craxii/current/craxii-workstation-reader
trusted_binary=/opt/craxii/current/craxii-server
trusted_admin=/opt/craxii/current/craxii-admin
trusted_binary_digest="$(sha256sum "${trusted_binary}")"

printf '%s\n' synthetic-provider-canary >"${credential}"
printf '%s\n' synthetic-config-canary >"${config}"
printf '%s\n' synthetic-state-canary >"${state}"
printf '%s\n' workspace-readable >"${workspace}"
chown root:root "${credential}"
chmod 0600 "${credential}"
chown root:craxii-server "${config}"
chmod 0640 "${config}"
chown craxii-server:craxii-server "${state}"
chmod 0600 "${state}"
chown craxii:craxii-server "${workspace}"
chmod 0660 "${workspace}"

[[ "$(stat -c '%U:%G:%a' "${launcher}")" == "root:craxii-server:4750" ]]
[[ "$(stat -c '%U:%G:%a' "${reader}")" == "root:root:111" ]]
[[ "$(stat -c '%U:%G:%a' "${trusted_binary}")" == "root:craxii-server:550" ]]
[[ "$(stat -c '%U:%G:%a' "${trusted_admin}")" == "root:craxii-server:550" ]]
[[ "$(id -G craxii | wc -w)" -eq 1 ]]
if runuser -u craxii -- "${launcher}" shell x y true >/dev/null 2>&1; then
  echo "error: workstation user invoked the privileged launcher" >&2
  exit 1
fi
if "${launcher}" shell x y true >/dev/null 2>&1; then
  echo "error: root bypassed the fixed real-caller check" >&2
  exit 1
fi

runuser -u craxii-server -- /usr/bin/env CRAXII_BACKEND_CANARY=synthetic-environment-canary \
  /bin/bash --noprofile --norc -c \
  'exec 9<"$1"; exec "$2" shell 00000000-0000-7000-8000-000000000000 00000000-0000-7000-8000-000000000000 "$3"' \
  stage27-server "${state}" "${launcher}" \
  "test \"\$(id -un)\" = craxii; \
   test \"\$(id -gn)\" = craxii; \
   grep -Eq '^Groups:[[:space:]]*$' /proc/self/status; \
   grep -Eq '^CapEff:[[:space:]]*0+$' /proc/self/status; \
   grep -Eq '^CapAmb:[[:space:]]*0+$' /proc/self/status; \
   grep -Eq '^NoNewPrivs:[[:space:]]*1$' /proc/self/status; \
   test ! -e /proc/self/fd/9; \
   test -z \"\${CRAXII_BACKEND_CANARY-}\"; \
   test \"\$HOME\" = /home/craxii; test \"\$USER\" = craxii; \
   test \"\$LOGNAME\" = craxii; test \"\$SHELL\" = /bin/bash; \
   test \"\$LANG\" = C.UTF-8; \
   test \"\$PATH\" = /home/craxii/.local/bin:/home/craxii/.cargo/bin:/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin; \
   ! cat '${credential}' >/dev/null 2>&1; \
   ! cat '${config}' >/dev/null 2>&1; \
   ! cat '${state}' >/dev/null 2>&1; \
   ! cat '${trusted_binary}' >/dev/null 2>&1; \
   ! cat '${trusted_admin}' >/dev/null 2>&1; \
   ! cat '${reader}' >/dev/null 2>&1; \
   ! cat '${launcher}' >/dev/null 2>&1; \
   ! '${trusted_binary}' --config '${config}' >/dev/null 2>&1; \
   ! '${trusted_admin}' --config '${config}' preflight >/dev/null 2>&1; \
   ! '${launcher}' shell x y true >/dev/null 2>&1; \
   printf changed >'${workspace}'; \
   test \"\$(id -u)\" != 0; \
   printf stage27-linux-shell-ok" \
  | grep -qx stage27-linux-shell-ok

[[ "$(runuser -u craxii-server -- cat "${workspace}")" == "changed" ]]
workspace_device="$(stat -c %d "${workspace}")"
workspace_inode="$(stat -c %i "${workspace}")"
[[ "$(runuser -u craxii-server -- "${launcher}" read-file "${workspace}" 1024 "${workspace_device}" "${workspace_inode}")" == "changed" ]]

credential_device="$(stat -c %d "${credential}")"
credential_inode="$(stat -c %i "${credential}")"
set +e
runuser -u craxii-server -- "${launcher}" read-file \
  "${credential}" 1024 "${credential_device}" "${credential_inode}" >/dev/null 2>&1
credential_read_status=$?
set -e
if [[ "${credential_read_status}" -eq 0 ]]; then
  echo "error: dropped reader accessed the synthetic provider credential" >&2
  exit 1
fi
[[ "${credential_read_status}" -eq 67 ]]

state_device="$(stat -c %d "${state}")"
state_inode="$(stat -c %i "${state}")"
set +e
runuser -u craxii-server -- "${launcher}" read-file \
  "${state}" 1024 "${state_device}" "${state_inode}" >/dev/null 2>&1
state_read_status=$?
set -e
[[ "${state_read_status}" -eq 67 ]]

mount -o remount,rw /sys/fs/cgroup
cgroup_root=/sys/fs/cgroup/craxii-stage27
mkdir "${cgroup_root}"
chown craxii-server:craxii-server \
  "${cgroup_root}" \
  "${cgroup_root}/cgroup.procs" \
  "${cgroup_root}/cgroup.subtree_control"

cargo test --locked -p craxii-server --lib --no-run
test_binary="$(find /craxii-target/debug/deps -maxdepth 1 -type f -perm -0100 \
  -name 'craxii_server-*' -print | sort | tail -n 1)"
[[ -n "${test_binary}" ]]
server_uid="$(id -u craxii-server)"
server_gid="$(id -g craxii-server)"
(
  # Match systemd Delegate=yes: the backend starts inside the delegated parent, so it can move
  # its children into execution sub-cgroups without write access to the host cgroup root.
  printf '0\n' >"${cgroup_root}/cgroup.procs"
  exec /usr/bin/setpriv \
    --reuid="${server_uid}" \
    --regid="${server_gid}" \
    --clear-groups \
    --inh-caps=+kill \
    --ambient-caps=+kill \
    /usr/bin/env \
    OPENAI_API_KEY=synthetic-provider-environment-canary \
    CREDENTIALS_DIRECTORY=/synthetic/systemd-credentials \
    AWS_ACCESS_KEY_ID=synthetic-aws-access-key-canary \
    AWS_SECRET_ACCESS_KEY=synthetic-aws-environment-canary \
    AWS_SESSION_TOKEN=synthetic-aws-session-canary \
    CRAXII_BACKEND_CANARY=synthetic-backend-environment-canary \
    CRAXII_BACKEND_AUTH_CANARY=synthetic-backend-auth-canary \
    SSM_ENVIRONMENT_CANARY=synthetic-ssm-environment-canary \
    CRAXII_STAGE27_USER_SWITCH_LAUNCHER="${launcher}" \
    CRAXII_STAGE27_CGROUP_ROOT="${cgroup_root}" \
    CRAXII_STAGE27_PROTECTED_CREDENTIAL="${credential}" \
    CRAXII_STAGE27_PROTECTED_CONFIG="${config}" \
    CRAXII_STAGE27_PROTECTED_STATE="${state}" \
    CRAXII_STAGE27_TRUSTED_BINARY="${trusted_binary}" \
    "${test_binary}" \
    --exact adapters::local_workstation::tests::linux_stage27_user_switch_isolates_identity_files_fds_and_process_lifecycle \
    --ignored
)
[[ "$(<"${credential}")" == "synthetic-provider-canary" ]]
[[ "$(<"${config}")" == "synthetic-config-canary" ]]
[[ "$(<"${state}")" == "synthetic-state-canary" ]]
[[ "$(sha256sum "${trusted_binary}")" == "${trusted_binary_digest}" ]]
rmdir "${cgroup_root}"

echo "STAGE27_DISPOSABLE_LINUX_IDENTITY_BOUNDARY: PASSED"
