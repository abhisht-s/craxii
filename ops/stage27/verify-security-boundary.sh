#!/usr/bin/env bash
set -euo pipefail

if (( EUID != 0 )); then
  echo "error: run as root on the explicitly authorized Stage 27 host" >&2
  exit 1
fi

launcher=/opt/craxii/current/craxii-workstation-launcher
reader=/opt/craxii/current/craxii-workstation-reader
trusted_binary=/opt/craxii/current/craxii-server
trusted_binary_digest="$(sha256sum "${trusted_binary}")"
credential_directory=/etc/craxii/credentials
provider_credential="${credential_directory}/openai_provider"
workspace=/srv/craxii/workspaces/primary
synthetic_id="stage27-$$"
synthetic_credential="${credential_directory}/.${synthetic_id}-credential"
synthetic_config="/etc/craxii/.${synthetic_id}-config"
synthetic_state="/var/lib/craxii/.${synthetic_id}-state"
synthetic_workspace="${workspace}/.${synthetic_id}-workspace"
backend_pid_file="/run/craxii/.${synthetic_id}-backend-pid"
backend_pid=""
backend_runner_pid=""

cleanup() {
  if [[ -n "${backend_pid}" ]]; then
    kill "${backend_pid}" 2>/dev/null || true
  fi
  if [[ -n "${backend_runner_pid}" ]]; then
    kill "${backend_runner_pid}" 2>/dev/null || true
    wait "${backend_runner_pid}" 2>/dev/null || true
  fi
  rm -f "${synthetic_credential}" "${synthetic_config}" "${synthetic_state}" \
    "${synthetic_workspace}" "${backend_pid_file}"
}
trap cleanup EXIT

[[ ! -e "${provider_credential}" ]]
[[ "$(id -un craxii-server)" == "craxii-server" ]]
[[ "$(id -gn craxii-server)" == "craxii-server" ]]
[[ "$(id -un craxii)" == "craxii" ]]
[[ "$(id -gn craxii)" == "craxii" ]]
[[ "$(id -G craxii | wc -w)" -eq 1 ]]
! id -nG craxii | tr ' ' '\n' | grep -Eq '^(sudo|docker|craxii-server)$'

[[ "$(stat -c '%U:%G:%a' "${launcher}")" == "root:craxii-server:4750" ]]
[[ "$(stat -c '%U:%G:%a' "${reader}")" == "root:root:555" ]]
[[ "$(stat -c '%U:%G:%a' "${trusted_binary}")" == "root:root:555" ]]
[[ "$(stat -c '%U:%G:%a' /var/lib/craxii)" == "craxii-server:craxii-server:700" ]]
[[ "$(stat -c '%U:%G:%a' "${credential_directory}")" == "root:root:700" ]]
grep -qx 'User=craxii-server' /etc/systemd/system/craxii-server.service
grep -qx 'Group=craxii-server' /etc/systemd/system/craxii-server.service
grep -qx 'Delegate=yes' /etc/systemd/system/craxii-server.service
grep -qx 'KillMode=control-group' /etc/systemd/system/craxii-server.service
grep -qx 'Restart=on-failure' /etc/systemd/system/craxii-server.service
grep -qx 'AmbientCapabilities=CAP_KILL' /etc/systemd/system/craxii-server.service
systemd-analyze verify /etc/systemd/system/craxii-server.service

printf '%s\n' 'synthetic-provider-canary' >"${synthetic_credential}"
printf '%s\n' 'synthetic-config-canary' >"${synthetic_config}"
printf '%s\n' 'synthetic-state-canary' >"${synthetic_state}"
printf '%s\n' 'workspace-readable' >"${synthetic_workspace}"
chown root:root "${synthetic_credential}"
chmod 0600 "${synthetic_credential}"
chown root:craxii-server "${synthetic_config}"
chmod 0640 "${synthetic_config}"
chown craxii-server:craxii-server "${synthetic_state}"
chmod 0600 "${synthetic_state}"
chown craxii:craxii "${synthetic_workspace}"
chmod 0600 "${synthetic_workspace}"
setfacl -m u:craxii-server:rw "${synthetic_workspace}"

runuser -u craxii-server -- /usr/bin/env -i \
  CRAXII_STAGE27_BACKEND_CANARY=synthetic-environment-canary \
  /bin/bash --noprofile --norc -c \
  'exec 9<"$1"; printf "%s\n" "$$" >"$2"; exec /bin/sleep 60' \
  stage27-backend "${synthetic_state}" "${backend_pid_file}" &
backend_runner_pid=$!
for _ in {1..50}; do
  [[ -s "${backend_pid_file}" ]] && break
  /bin/sleep 0.1
done
[[ -s "${backend_pid_file}" ]]
observed_backend_pid="$(<"${backend_pid_file}")"
backend_pid="${observed_backend_pid}"
[[ "$(stat -c %U "/proc/${observed_backend_pid}")" == "craxii-server" ]]

shell_probe=$(printf '%q ' \
  "${synthetic_credential}" "${synthetic_config}" "${synthetic_state}" \
  "${trusted_binary}" "${launcher}" "${synthetic_workspace}" "${observed_backend_pid}")
shell_probe="set -- ${shell_probe}; \
test \"\$(id -un)\" = craxii; test \"\$(id -gn)\" = craxii; \
grep -Eq '^Groups:[[:space:]]*$' /proc/self/status; \
grep -Eq '^CapEff:[[:space:]]*0+$' /proc/self/status; \
grep -Eq '^CapAmb:[[:space:]]*0+$' /proc/self/status; \
grep -Eq '^NoNewPrivs:[[:space:]]*1$' /proc/self/status; \
test \"\$HOME\" = /home/craxii; test \"\$USER\" = craxii; \
test \"\$LOGNAME\" = craxii; test \"\$SHELL\" = /bin/bash; \
test \"\$LANG\" = C.UTF-8; \
test \"\$PATH\" = /home/craxii/.local/bin:/home/craxii/.cargo/bin:/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin; \
test -n \"\${CRAXII_WORK_ID-}\"; test -n \"\${CRAXII_WORKSPACE_ID-}\"; \
test -z \"\${CRAXII_STAGE27_BACKEND_CANARY-}\"; \
test -z \"\${CREDENTIALS_DIRECTORY-}\"; \
test -z \"\${CRAXII_BACKEND_AUTH_CANARY-}\"; \
! cat \"\$1\" >/dev/null 2>&1; ! cat \"\$2\" >/dev/null 2>&1; \
! cat \"\$3\" >/dev/null 2>&1; \
! /bin/bash --noprofile --norc -c \"printf x >>'\$4'\" 2>/dev/null; \
! \"\$5\" shell x y true >/dev/null 2>&1; \
! cat /proc/\"\$7\"/environ >/dev/null 2>&1; \
test ! -e /proc/self/fd/9; \
printf changed >\"\$6\"; \
if command -v sudo >/dev/null; then ! sudo -n /bin/true >/dev/null 2>&1; fi; \
test \"\$(id -u)\" != 0; printf stage27-shell-ok"

shell_output=$(runuser -u craxii-server -- /usr/bin/env -i \
  CRAXII_STAGE27_BACKEND_CANARY=synthetic-environment-canary \
  CREDENTIALS_DIRECTORY=/synthetic/systemd-credentials \
  CRAXII_BACKEND_AUTH_CANARY=synthetic-backend-auth-canary \
  /bin/bash --noprofile --norc -c \
  'exec 9<"$1"; exec "$2" shell 00000000-0000-7000-8000-000000000000 00000000-0000-7000-8000-000000000000 "$3"' \
  stage27-server "${synthetic_state}" "${launcher}" "${shell_probe}")
[[ "${shell_output}" == "stage27-shell-ok" ]]
[[ "$(<"${synthetic_workspace}")" == "changed" ]]
[[ "$(runuser -u craxii-server -- /bin/cat "${synthetic_workspace}")" == "changed" ]]
[[ "$(<"${synthetic_credential}")" == "synthetic-provider-canary" ]]
[[ "$(<"${synthetic_config}")" == "synthetic-config-canary" ]]
[[ "$(<"${synthetic_state}")" == "synthetic-state-canary" ]]
[[ "$(sha256sum "${trusted_binary}")" == "${trusted_binary_digest}" ]]

workspace_device="$(stat -c %d "${synthetic_workspace}")"
workspace_inode="$(stat -c %i "${synthetic_workspace}")"
reader_output=$(runuser -u craxii-server -- "${launcher}" read-file \
  "${synthetic_workspace}" 1024 "${workspace_device}" "${workspace_inode}")
[[ "${reader_output}" == "changed" ]]

credential_device="$(stat -c %d "${synthetic_credential}")"
credential_inode="$(stat -c %i "${synthetic_credential}")"
set +e
runuser -u craxii-server -- "${launcher}" read-file \
  "${synthetic_credential}" 1024 "${credential_device}" "${credential_inode}" \
  >/dev/null 2>&1
credential_read_status=$?
set -e
if [[ "${credential_read_status}" -eq 0 ]]; then
  echo "error: workstation reader accessed the synthetic credential" >&2
  exit 1
fi
[[ "${credential_read_status}" -eq 67 ]]

state_device="$(stat -c %d "${synthetic_state}")"
state_inode="$(stat -c %i "${synthetic_state}")"
set +e
runuser -u craxii-server -- "${launcher}" read-file \
  "${synthetic_state}" 1024 "${state_device}" "${state_inode}" >/dev/null 2>&1
state_read_status=$?
set -e
[[ "${state_read_status}" -eq 67 ]]

echo "Stage 27 synthetic Linux identity and filesystem boundary passed."
