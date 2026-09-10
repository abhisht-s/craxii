#!/usr/bin/env bash
set -euo pipefail

fail() {
  echo "error: $*" >&2
  exit 1
}

(( EUID == 0 )) || fail "run as root on the explicitly authorized Stage 27 host"
[[ $# -eq 1 && "$1" =~ ^[0-9a-f]{40}$ ]] ||
  fail "usage: $0 <exact-40-character-commit>"

readonly commit="$1"
readonly release_version="0.0.1-${commit:0:12}"
readonly checkout=/var/lib/craxii-build/source
readonly source_directory=/var/lib/craxii-build/target/release
readonly release_directory="/opt/craxii/releases/${release_version}"
readonly current=/opt/craxii/current
readonly service=craxii-server.service
readonly asset_directory="${checkout}/ops/stage27"
readonly installed_config=/etc/craxii/config.toml
readonly installed_unit=/etc/systemd/system/craxii-server.service
staging_directory=""
pending_config=""
pending_unit=""
temporary_link=""
candidate_start_attempted=0
deployment_complete=0

cleanup() {
  local status=$?
  trap - EXIT
  set +e
  if [[ -n "${staging_directory}" &&
        "${staging_directory}" == /run/craxii-stage27-upgrade-[0-9]* ]]; then
    rm -rf -- "${staging_directory}"
  fi
  if [[ -n "${pending_config}" &&
        "${pending_config}" == /etc/craxii/.config.toml.0.0.1-[0-9a-f]*.[0-9]* ]]; then
    rm -f -- "${pending_config}"
  fi
  if [[ -n "${pending_unit}" &&
        "${pending_unit}" == /etc/systemd/system/.craxii-server.service.0.0.1-[0-9a-f]*.[0-9]* ]]; then
    rm -f -- "${pending_unit}"
  fi
  if [[ -n "${temporary_link}" &&
        "${temporary_link}" == /opt/craxii/.current-0.0.1-[0-9a-f]*-[0-9]* ]]; then
    rm -f -- "${temporary_link}"
  fi
  if [[ "${status}" -ne 0 && "${candidate_start_attempted}" -eq 1 &&
        "${deployment_complete}" -eq 0 ]]; then
    # The candidate may already have applied the forward-only schema migration. Never restart the
    # previous binary after this point, but do stop an unverified candidate or its restart loop.
    systemctl stop "${service}" 2>/dev/null || true
  fi
  exit "${status}"
}
trap cleanup EXIT

build_git() {
  runuser -u craxii-build -- /usr/bin/env -i \
    HOME=/var/lib/craxii-build \
    PATH=/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin \
    /usr/bin/git -C "${checkout}" "$@"
}

[[ "$(build_git rev-parse HEAD)" == "${commit}" ]] ||
  fail "build checkout does not match the requested commit"
[[ -z "$(build_git status --porcelain=v1 --untracked-files=normal)" ]] ||
  fail "build checkout is dirty"
"${asset_directory}/verify-release-manifest.sh" "${source_directory}" "${commit}" >/dev/null
[[ -f "${asset_directory}/config.toml.template" &&
   ! -L "${asset_directory}/config.toml.template" ]] ||
  fail "candidate production config is absent or unsafe"
[[ -f "${asset_directory}/craxii-server.service" &&
   ! -L "${asset_directory}/craxii-server.service" ]] ||
  fail "candidate systemd unit is absent or unsafe"
[[ -L "${current}" ]] || fail "active release symlink is absent"
previous_release="$(readlink -f "${current}")"
[[ "${previous_release}" == /opt/craxii/releases/* ]] ||
  fail "active release is outside the immutable release root"
[[ "${previous_release}" != "${release_directory}" ]] ||
  fail "requested release is already active"
[[ ! -e "${release_directory}" && ! -L "${release_directory}" ]] ||
  fail "immutable release path already exists"
systemctl is-active --quiet "${service}" || fail "service is not active"
systemctl is-enabled --quiet "${service}" || fail "service is not enabled"

# Prepare and validate every mutable host asset before stopping the healthy release. The final
# same-filesystem renames below prevent a short or partially copied config/unit from becoming the
# installed candidate.
staging_directory="/run/craxii-stage27-upgrade-$$"
[[ ! -e "${staging_directory}" && ! -L "${staging_directory}" ]] ||
  fail "deployment staging path already exists"
install -d -o root -g root -m 0700 "${staging_directory}"
readonly staged_config="${staging_directory}/config.toml"
readonly staged_unit="${staging_directory}/craxii-server.service"
pending_config="/etc/craxii/.config.toml.${release_version}.$$"
pending_unit="/etc/systemd/system/.craxii-server.service.${release_version}.$$"
install -o root -g craxii-server -m 0640 \
  "${asset_directory}/config.toml.template" "${staged_config}"
install -o root -g root -m 0644 \
  "${asset_directory}/craxii-server.service" "${staged_unit}"
cmp -s "${asset_directory}/config.toml.template" "${staged_config}" ||
  fail "staged production config differs from the candidate"
cmp -s "${asset_directory}/craxii-server.service" "${staged_unit}" ||
  fail "staged systemd unit differs from the candidate"
systemd-analyze verify "${staged_unit}"
install -o root -g craxii-server -m 0640 "${staged_config}" "${pending_config}"
install -o root -g root -m 0644 "${staged_unit}" "${pending_unit}"

for binary in \
  craxii-server craxii-admin craxii-stage27-luna-benchmark \
  craxii-workstation-launcher craxii-workstation-reader; do
  [[ -f "${source_directory}/${binary}" && -x "${source_directory}/${binary}" ]] ||
    fail "built release binary is absent: ${binary}"
done

install -d -o root -g root -m 0755 "${release_directory}"
install -o root -g craxii-server -m 0550 \
  "${source_directory}/craxii-server" "${release_directory}/craxii-server"
install -o root -g craxii-server -m 0550 \
  "${source_directory}/craxii-admin" "${release_directory}/craxii-admin"
install -o root -g craxii-server -m 0550 \
  "${source_directory}/craxii-stage27-luna-benchmark" \
  "${release_directory}/craxii-stage27-luna-benchmark"
install -o root -g root -m 0111 \
  "${source_directory}/craxii-workstation-reader" \
  "${release_directory}/craxii-workstation-reader"
install -o root -g craxii-server -m 4750 \
  "${source_directory}/craxii-workstation-launcher" \
  "${release_directory}/craxii-workstation-launcher"
install -o root -g root -m 0444 \
  "${source_directory}/.craxii-stage27-build-manifest" \
  "${release_directory}/.craxii-stage27-build-manifest"

for binary in \
  craxii-server craxii-admin craxii-stage27-luna-benchmark \
  craxii-workstation-launcher craxii-workstation-reader; do
  [[ "$(sha256sum "${source_directory}/${binary}" | cut -d' ' -f1)" \
      == "$(sha256sum "${release_directory}/${binary}" | cut -d' ' -f1)" ]] ||
    fail "installed release digest mismatch: ${binary}"
done
"${asset_directory}/verify-release-manifest.sh" \
  "${release_directory}" "${commit}" >/dev/null

systemctl stop "${service}"
systemctl is-active --quiet "${service}" &&
  fail "service remained active after the deployment stop"
mv -Tf "${pending_config}" "${installed_config}"
mv -Tf "${pending_unit}" "${installed_unit}"
cmp -s "${asset_directory}/config.toml.template" "${installed_config}" ||
  fail "installed production config differs from the candidate"
cmp -s "${asset_directory}/craxii-server.service" "${installed_unit}" ||
  fail "installed systemd unit differs from the candidate"
[[ "$(stat -c '%U:%G:%a' "${installed_config}")" == root:craxii-server:640 ]] ||
  fail "installed production config metadata mismatch"
[[ "$(stat -c '%U:%G:%a' "${installed_unit}")" == root:root:644 ]] ||
  fail "installed systemd unit metadata mismatch"
systemctl daemon-reload
[[ "$(systemctl show "${service}" --property FragmentPath --value)" == "${installed_unit}" ]] ||
  fail "systemd loaded the service from an unexpected fragment"
[[ -z "$(systemctl show "${service}" --property DropInPaths --value)" ]] ||
  fail "unexpected systemd drop-in changes the candidate service"
[[ "$(systemctl show "${service}" --property NeedDaemonReload --value)" == no ]] ||
  fail "systemd manager state is stale after daemon-reload"
[[ "$(systemctl show "${service}" --property LimitCORE --value)" == 0 ]] ||
  fail "loaded service core limit differs from the candidate"
[[ "$(systemctl show "${service}" --property LimitCORESoft --value)" == 0 ]] ||
  fail "loaded service soft core limit differs from the candidate"
temporary_link="/opt/craxii/.current-${release_version}-$$"
ln -s "releases/${release_version}" "${temporary_link}"
mv -Tf "${temporary_link}" "${current}"
candidate_start_attempted=1
systemctl start "${service}"
systemctl is-active --quiet "${service}" || fail "updated service did not become active"
ready=0
for _ in {1..600}; do
  if [[ "$(curl --silent --output /dev/null --write-out '%{http_code}' \
    --connect-timeout 1 --max-time 2 http://127.0.0.1:8080/health/ready || true)" == 200 ]]; then
    ready=1
    break
  fi
  sleep 0.1
done
[[ "${ready}" -eq 1 ]] || fail "updated service did not become ready"
main_pid="$(systemctl show "${service}" --property MainPID --value)"
[[ "${main_pid}" =~ ^[1-9][0-9]*$ ]] || fail "updated service MainPID is invalid"
[[ "$(readlink -f "/proc/${main_pid}/exe")" == "${release_directory}/craxii-server" ]] ||
  fail "updated service does not execute the immutable release"
[[ "$(readlink -f "${current}")" == "${release_directory}" ]] ||
  fail "active release symlink does not identify the updated release"
deployment_complete=1

printf 'STAGE27_RELEASE_UPGRADE=PASS\n'
printf 'STAGE27_RELEASE=%s\n' "${release_directory}"
printf 'STAGE27_MAIN_PID=%s\n' "${main_pid}"
