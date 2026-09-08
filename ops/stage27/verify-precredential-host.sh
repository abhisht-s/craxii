#!/usr/bin/env bash
set -euo pipefail

fail() {
  echo "error: $*" >&2
  exit 1
}

(( EUID == 0 )) || fail "run as root on the explicitly authorized Stage 27 host"
if [[ $# -ne 1 || ! "$1" =~ ^[0-9a-f]{40}$ ]]; then
  echo "usage: $0 <exact-40-character-deployment-commit>" >&2
  exit 2
fi

readonly deployment_commit="$1"
readonly release_version="0.0.1-${deployment_commit:0:12}"
asset_directory="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
readonly asset_directory
readonly launcher=/opt/craxii/current/craxii-workstation-launcher
readonly reader=/opt/craxii/current/craxii-workstation-reader
readonly trusted_binary=/opt/craxii/current/craxii-server
readonly config=/etc/craxii/config.toml
readonly credential_directory=/etc/craxii/credentials
readonly provider_credential=${credential_directory}/openai_provider
readonly workspace=/srv/craxii/workspaces/primary
readonly source_directory=/var/lib/craxii-build/source
readonly synthetic_id="stage27-$$"
readonly synthetic_credential="${credential_directory}/.${synthetic_id}-credential"
readonly synthetic_state="/var/lib/craxii/.${synthetic_id}-state"
readonly synthetic_workspace="${workspace}/.${synthetic_id}-workspace"
readonly backend_pid_file="/run/craxii/.${synthetic_id}-backend-pid"
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
  rm -f "${synthetic_credential}" "${synthetic_state}" "${synthetic_workspace}" \
    "${backend_pid_file}"
}
trap cleanup EXIT

# Host, mount, package, checkout, and toolchain contract.
[[ "$(uname -m)" == x86_64 ]] || fail "host is not x86_64"
grep -qx 'ID=ubuntu' /etc/os-release || fail "host is not Ubuntu"
grep -qx 'VERSION_ID="24.04"' /etc/os-release || fail "host is not Ubuntu 24.04"
[[ "$(stat -fc %T /sys/fs/cgroup)" == cgroup2fs ]] || fail "cgroup v2 is not active"
"${asset_directory}/bootstrap-data-volume.sh" --verify-only
for package in acl bash build-essential ca-certificates curl e2fsprogs git procps sqlite3 util-linux; do
  dpkg-query -W -f='${Status}' "${package}" 2>/dev/null | grep -qx 'install ok installed' ||
    fail "required package is absent: ${package}"
done
[[ "$(runuser -u craxii-build -- /usr/bin/env -i \
  RUSTUP_HOME=/var/lib/craxii-build/rustup \
  CARGO_HOME=/var/lib/craxii-build/cargo \
  /var/lib/craxii-build/cargo/bin/rustc +1.98.0 --version)" == 'rustc 1.98.0 '* ]] ||
  fail "controlled Rust 1.98.0 toolchain is absent"
installed_components="$(runuser -u craxii-build -- /usr/bin/env -i \
  RUSTUP_HOME=/var/lib/craxii-build/rustup \
  CARGO_HOME=/var/lib/craxii-build/cargo \
  /var/lib/craxii-build/cargo/bin/rustup component list \
    --toolchain 1.98.0-x86_64-unknown-linux-gnu --installed)"
grep -q '^clippy-' <<<"${installed_components}" || fail "clippy is absent from Rust 1.98.0"
grep -q '^rustfmt-' <<<"${installed_components}" || fail "rustfmt is absent from Rust 1.98.0"
runuser -u craxii-build -- /usr/bin/env -i \
  RUSTUP_HOME=/var/lib/craxii-build/rustup \
  CARGO_HOME=/var/lib/craxii-build/cargo \
  /var/lib/craxii-build/cargo/bin/rustup target list \
    --toolchain 1.98.0-x86_64-unknown-linux-gnu --installed \
  | grep -qx 'x86_64-unknown-linux-gnu'

build_git() {
  runuser -u craxii-build -- /usr/bin/env -i \
    HOME=/var/lib/craxii-build \
    PATH=/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin \
    git -C "${source_directory}" "$@"
}
[[ "$(build_git rev-parse HEAD)" == "${deployment_commit}" ]] ||
  fail "build checkout does not match deployment commit"
[[ -z "$(build_git status --porcelain=v1 --untracked-files=normal)" ]] ||
  fail "build checkout is dirty"
[[ "$(readlink -f /opt/craxii/current)" == "/opt/craxii/releases/${release_version}" ]] ||
  fail "active release symlink does not match deployment commit"

# Exact service/config/release identities and precredential stop state.
[[ ! -e "${provider_credential}" && ! -L "${provider_credential}" ]] ||
  fail "real provider credential exists before the checkpoint"
! systemctl is-active --quiet craxii-server.service || fail "service is active before credential install"
! systemctl is-enabled --quiet craxii-server.service || fail "service is enabled before credential install"
[[ "$(id -un craxii-server)" == craxii-server ]]
[[ "$(id -gn craxii-server)" == craxii-server ]]
[[ "$(getent passwd craxii-server | cut -d: -f6-7)" == "/var/lib/craxii:/usr/sbin/nologin" ]]
[[ "$(id -un craxii)" == craxii ]]
[[ "$(id -gn craxii)" == craxii ]]
[[ "$(getent passwd craxii | cut -d: -f6-7)" == "/home/craxii:/bin/bash" ]]
[[ "$(id -G craxii-server | wc -w)" -eq 1 ]] || fail "craxii-server has supplementary groups"
[[ "$(id -G craxii | wc -w)" -eq 1 ]] || fail "craxii has supplementary groups"
[[ "$(passwd -S craxii-server | awk '{print $2}')" == L ]] || fail "craxii-server is not locked"
[[ "$(passwd -S craxii | awk '{print $2}')" == L ]] || fail "craxii is not locked"
[[ ! -e /home/craxii/.ssh/authorized_keys && ! -L /home/craxii/.ssh/authorized_keys ]] ||
  fail "craxii has an SSH authorized_keys file"

[[ "$(stat -c '%U:%G:%a' /opt/craxii)" == root:root:755 ]]
[[ "$(stat -c '%U:%G:%a' "${launcher}")" == root:craxii-server:4750 ]]
[[ "$(stat -c '%U:%G:%a:%h' "${launcher}")" == root:craxii-server:4750:1 ]]
[[ "$(stat -c '%U:%G:%a' "${reader}")" == root:root:555 ]]
[[ "$(stat -c '%U:%G:%a' "${trusted_binary}")" == root:root:555 ]]
[[ "$(stat -c '%U:%G:%a' /etc/craxii)" == root:craxii-server:750 ]]
[[ "$(stat -c '%U:%G:%a' "${config}")" == root:craxii-server:640 ]]
[[ "$(stat -c '%U:%G:%a' "${credential_directory}")" == craxii-server:craxii-server:700 ]]
[[ "$(stat -c '%U:%G:%a' /var/lib/craxii)" == craxii-server:craxii-server:700 ]]
[[ "$(stat -c '%U:%G:%a' /home/craxii)" == craxii:craxii:700 ]]
[[ "$(stat -c '%U:%G' "${workspace}")" == craxii:craxii ]]
getfacl -cp "${workspace}" | grep -qx 'user:craxii-server:r-x'
getfacl -cp "${workspace}" | grep -qx 'default:user:craxii-server:r-x'

grep -qx 'bind_address = "127.0.0.1:8080"' "${config}"
grep -qx 'state_root = "/var/lib/craxii"' "${config}"
grep -qx 'artifact_root = "/var/lib/craxii/artifacts"' "${config}"
grep -qx 'primary_workspace_root = "/srv/craxii/workspaces/primary"' "${config}"
grep -qx 'source = "systemd"' "${config}"
grep -qx 'default_target = "stage27-openai"' "${config}"
[[ "$(grep -c '^\[\[models.targets\]\]$' "${config}")" -eq 1 ]]
grep -qx 'provider = "openai"' "${config}"
grep -qx 'provider_model_id = "gpt-5.6-luna"' "${config}"
grep -qx 'administrative_enabled = false' "${config}"
grep -qx 'user_switch_launcher = "/opt/craxii/current/craxii-workstation-launcher"' "${config}"
grep -qx 'delegated_cgroup_root = "/sys/fs/cgroup/system.slice/craxii-server.service/craxii-executions"' "${config}"
if grep -q '__REQUIRED_' "${config}"; then
  fail "production config contains an unresolved placeholder"
fi
if grep -Eiq 'fallback|OPENAI_API_KEY|(^|[^[:alnum:]])sk-[A-Za-z0-9_-]{16,}' "${config}"; then
  fail "production config contains fallback or credential material"
fi

grep -qx 'User=craxii-server' /etc/systemd/system/craxii-server.service
grep -qx 'Group=craxii-server' /etc/systemd/system/craxii-server.service
grep -qx 'Delegate=yes' /etc/systemd/system/craxii-server.service
grep -qx 'KillMode=control-group' /etc/systemd/system/craxii-server.service
grep -qx 'Restart=on-failure' /etc/systemd/system/craxii-server.service
grep -qx 'KillSignal=SIGTERM' /etc/systemd/system/craxii-server.service
grep -qx 'AmbientCapabilities=CAP_KILL' /etc/systemd/system/craxii-server.service
grep -qx 'IPAddressDeny=169.254.169.254' /etc/systemd/system/craxii-server.service
grep -qx 'IPAddressDeny=fd00:ec2::254' /etc/systemd/system/craxii-server.service
grep -qx 'LoadCredential=openai_provider:/etc/craxii/credentials/openai_provider' \
  /etc/systemd/system/craxii-server.service
if grep -Eq '^Environment(File)?=' /etc/systemd/system/craxii-server.service; then
  fail "systemd unit contains a global environment source"
fi
systemd-analyze verify /etc/systemd/system/craxii-server.service

# No real OpenAI credential is accepted in known host configuration or any current process.
if grep -RqsE 'OPENAI_API_KEY|(^|[^[:alnum:]])sk-[A-Za-z0-9_-]{16,}' \
  /etc/craxii /etc/environment /etc/profile /etc/profile.d 2>/dev/null; then
  fail "OpenAI credential material was found in host configuration"
fi
while IFS= read -r process_environment; do
  if tr '\0' '\n' <"${process_environment}" 2>/dev/null | grep -q '^OPENAI_API_KEY='; then
    fail "OPENAI_API_KEY is present in a running process environment"
  fi
done < <(find /proc -maxdepth 2 -path '/proc/[0-9]*/environ' -type f -print 2>/dev/null)

# Synthetic-only boundary probes. Nothing below is a provider credential or external request.
trusted_binary_digest="$(sha256sum "${trusted_binary}")"
config_digest="$(sha256sum "${config}")"
printf '%s\n' synthetic-provider-canary >"${synthetic_credential}"
printf '%s\n' synthetic-state-canary >"${synthetic_state}"
chown craxii-server:craxii-server "${synthetic_credential}" "${synthetic_state}"
chmod 0600 "${synthetic_credential}" "${synthetic_state}"
# Positional arguments intentionally expand in the inner shell.
# shellcheck disable=SC2016
runuser -u craxii -- /usr/bin/env -i PATH=/usr/bin:/bin \
  /bin/bash --noprofile --norc -c \
  'umask 077; printf "%s\n" workspace-readable >"$1"' stage27-workspace "${synthetic_workspace}"
setfacl -m u:craxii-server:r-- "${synthetic_workspace}"

# Positional arguments intentionally expand in the inner shell.
# shellcheck disable=SC2016
runuser -u craxii-server -- /usr/bin/env -i \
  CRAXII_STAGE27_BACKEND_CANARY=synthetic-environment-canary \
  OPENAI_API_KEY=synthetic-openai-environment-canary \
  AWS_ACCESS_KEY_ID=synthetic-aws-access-key-canary \
  AWS_SECRET_ACCESS_KEY=synthetic-aws-secret-canary \
  AWS_SESSION_TOKEN=synthetic-aws-session-canary \
  /bin/bash --noprofile --norc -c \
  'exec 9<"$1"; printf "%s\n" "$$" >"$2"; exec /bin/sleep 60' \
  stage27-backend "${synthetic_state}" "${backend_pid_file}" &
backend_runner_pid=$!
for _ in {1..50}; do
  [[ -s "${backend_pid_file}" ]] && break
  /bin/sleep 0.1
done
[[ -s "${backend_pid_file}" ]] || fail "synthetic backend did not start"
observed_backend_pid="$(<"${backend_pid_file}")"
backend_pid="${observed_backend_pid}"
[[ "$(stat -c %U "/proc/${observed_backend_pid}")" == craxii-server ]]

shell_probe=$(printf '%q ' \
  "${synthetic_credential}" "${config}" "${synthetic_state}" \
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
test -z \"\${CREDENTIALS_DIRECTORY-}\"; test -z \"\${CRAXII_BACKEND_AUTH_CANARY-}\"; \
test -z \"\${OPENAI_API_KEY-}\"; test -z \"\${AWS_ACCESS_KEY_ID-}\"; \
test -z \"\${AWS_SECRET_ACCESS_KEY-}\"; test -z \"\${AWS_SESSION_TOKEN-}\"; \
test -z \"\${AWS_PROFILE-}\"; test -z \"\${AWS_SHARED_CREDENTIALS_FILE-}\"; \
test -z \"\${AWS_WEB_IDENTITY_TOKEN_FILE-}\"; \
test -z \"\${AWS_CONTAINER_CREDENTIALS_RELATIVE_URI-}\"; \
! cat \"\$1\" >/dev/null 2>&1; ! cat \"\$2\" >/dev/null 2>&1; \
! cat \"\$3\" >/dev/null 2>&1; \
! /bin/bash --noprofile --norc -c \"printf x >>'\$4'\" 2>/dev/null; \
! \"\$5\" shell x y true >/dev/null 2>&1; \
! cat /proc/\"\$7\"/environ >/dev/null 2>&1; \
test ! -e /proc/self/fd/9; printf changed >\"\$6\"; \
if command -v sudo >/dev/null; then ! sudo -n /bin/true >/dev/null 2>&1; fi; \
test \"\$(id -u)\" != 0; printf stage27-shell-ok"

server_uid="$(id -u craxii-server)"
server_gid="$(id -g craxii-server)"
# Positional arguments intentionally expand in the inner shell.
# shellcheck disable=SC2016
shell_output=$(/usr/bin/setpriv \
  --reuid="${server_uid}" --regid="${server_gid}" --clear-groups \
  --inh-caps=+kill --ambient-caps=+kill \
  /usr/bin/env -i \
  CRAXII_STAGE27_BACKEND_CANARY=synthetic-environment-canary \
  CREDENTIALS_DIRECTORY=/synthetic/systemd-credentials \
  CRAXII_BACKEND_AUTH_CANARY=synthetic-backend-auth-canary \
  OPENAI_API_KEY=synthetic-openai-environment-canary \
  AWS_ACCESS_KEY_ID=synthetic-aws-access-key-canary \
  AWS_SECRET_ACCESS_KEY=synthetic-aws-secret-canary \
  AWS_SESSION_TOKEN=synthetic-aws-session-canary \
  AWS_PROFILE=synthetic-aws-profile-canary \
  AWS_SHARED_CREDENTIALS_FILE=/synthetic/aws-credentials \
  AWS_WEB_IDENTITY_TOKEN_FILE=/synthetic/aws-web-token \
  AWS_CONTAINER_CREDENTIALS_RELATIVE_URI=/synthetic/aws-container-credentials \
  /bin/bash --noprofile --norc -c \
  'grep -Eq "^CapAmb:[[:space:]]*0*20$" /proc/self/status; exec 9<"$1"; \
   exec "$2" shell 00000000-0000-7000-8000-000000000000 \
   00000000-0000-7000-8000-000000000000 "$3"' \
  stage27-server "${synthetic_state}" "${launcher}" "${shell_probe}")
[[ "${shell_output}" == stage27-shell-ok ]]
[[ "$(<"${synthetic_workspace}")" == changed ]]
[[ "$(runuser -u craxii-server -- /bin/cat "${synthetic_workspace}")" == changed ]]
# Positional arguments intentionally expand in the inner shell.
# shellcheck disable=SC2016
if runuser -u craxii-server -- /bin/bash --noprofile --norc -c \
  'printf forbidden >>"$1"' stage27-backend "${synthetic_workspace}" 2>/dev/null; then
  fail "backend has write access to model workspace evidence"
fi
[[ "$(<"${synthetic_credential}")" == synthetic-provider-canary ]]
[[ "$(<"${synthetic_state}")" == synthetic-state-canary ]]
[[ "$(sha256sum "${trusted_binary}")" == "${trusted_binary_digest}" ]]
[[ "$(sha256sum "${config}")" == "${config_digest}" ]]

workspace_device="$(stat -c %d "${synthetic_workspace}")"
workspace_inode="$(stat -c %i "${synthetic_workspace}")"
reader_output="$(runuser -u craxii-server -- "${launcher}" read-file \
  "${synthetic_workspace}" 1024 "${workspace_device}" "${workspace_inode}")"
[[ "${reader_output}" == changed ]]

for protected_path in "${synthetic_credential}" "${config}" "${synthetic_state}"; do
  protected_device="$(stat -c %d "${protected_path}")"
  protected_inode="$(stat -c %i "${protected_path}")"
  set +e
  runuser -u craxii-server -- "${launcher}" read-file \
    "${protected_path}" 131072 "${protected_device}" "${protected_inode}" >/dev/null 2>&1
  protected_read_status=$?
  set -e
  [[ "${protected_read_status}" -eq 67 ]] ||
    fail "workstation reader accessed protected path: ${protected_path}"
done

if runuser -u craxii -- "${launcher}" shell x y true >/dev/null 2>&1; then
  fail "workstation user invoked the privileged launcher"
fi
if "${launcher}" shell x y true >/dev/null 2>&1; then
  fail "root bypassed the fixed real-caller check"
fi

echo "Stage 27 precredential Ubuntu host, storage, release, service, and synthetic boundary passed."
echo "STAGE27_PRECREDENTIAL_HOST: PASSED"
