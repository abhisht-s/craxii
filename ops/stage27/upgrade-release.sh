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

systemctl stop "${service}"
systemctl is-active --quiet "${service}" &&
  fail "service remained active after the deployment stop"
temporary_link="/opt/craxii/.current-${release_version}-$$"
ln -s "releases/${release_version}" "${temporary_link}"
mv -Tf "${temporary_link}" "${current}"
systemctl start "${service}"
systemctl is-active --quiet "${service}" || fail "updated service did not become active"
main_pid="$(systemctl show "${service}" --property MainPID --value)"
[[ "${main_pid}" =~ ^[1-9][0-9]*$ ]] || fail "updated service MainPID is invalid"
[[ "$(readlink -f "/proc/${main_pid}/exe")" == "${release_directory}/craxii-server" ]] ||
  fail "updated service does not execute the immutable release"
[[ "$(readlink -f "${current}")" == "${release_directory}" ]] ||
  fail "active release symlink does not identify the updated release"

printf 'STAGE27_RELEASE_UPGRADE=PASS\n'
printf 'STAGE27_RELEASE=%s\n' "${release_directory}"
printf 'STAGE27_MAIN_PID=%s\n' "${main_pid}"
