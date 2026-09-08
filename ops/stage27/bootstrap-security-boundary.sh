#!/usr/bin/env bash
set -euo pipefail

if (( EUID != 0 )); then
  echo "error: run as root on the explicitly authorized Stage 27 host" >&2
  exit 1
fi
if [[ $# -ne 2 ]]; then
  echo "usage: $0 <built-release-directory> <release-version>" >&2
  exit 2
fi

source_directory="$1"
release_version="$2"
asset_directory="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"

fail() {
  echo "error: $*" >&2
  exit 1
}

[[ "${release_version}" =~ ^[A-Za-z0-9._-]+$ ]]
[[ "$(uname -m)" == "x86_64" ]]
grep -qx 'ID=ubuntu' /etc/os-release
grep -qx 'VERSION_ID="24.04"' /etc/os-release
for command in groupadd useradd getent install setfacl sha256sum runuser setpriv; do
  command -v "${command}" >/dev/null
done
for binary in craxii-server craxii-admin craxii-workstation-launcher craxii-workstation-reader; do
  [[ -f "${source_directory}/${binary}" && -x "${source_directory}/${binary}" ]]
done
"${asset_directory}/bootstrap-data-volume.sh" --verify-only
[[ ! -e /etc/craxii/credentials/openai_provider ]] ||
  fail "provider credential already exists; precredential bootstrap refused"
if systemctl is-active --quiet craxii-server.service 2>/dev/null ||
  systemctl is-enabled --quiet craxii-server.service 2>/dev/null; then
  fail "craxii-server.service must be stopped and disabled before bootstrap"
fi

ensure_group() {
  local group="$1"
  getent group "${group}" >/dev/null || groupadd --system "${group}"
}

ensure_group craxii-server
ensure_group craxii

if ! getent passwd craxii-server >/dev/null; then
  useradd --system --gid craxii-server --home-dir /var/lib/craxii \
    --no-create-home --shell /usr/sbin/nologin craxii-server
fi
if ! getent passwd craxii >/dev/null; then
  useradd --gid craxii --no-create-home --home-dir /home/craxii --shell /bin/bash craxii
fi

usermod -G '' craxii-server
usermod -G '' craxii
passwd -l craxii-server >/dev/null
passwd -l craxii >/dev/null

[[ "$(id -un craxii-server)" == "craxii-server" ]]
[[ "$(id -gn craxii-server)" == "craxii-server" ]]
[[ "$(getent passwd craxii-server | cut -d: -f6-7)" == "/var/lib/craxii:/usr/sbin/nologin" ]]
[[ "$(id -un craxii)" == "craxii" ]]
[[ "$(id -gn craxii)" == "craxii" ]]
[[ "$(getent passwd craxii | cut -d: -f6-7)" == "/home/craxii:/bin/bash" ]]
[[ "$(id -G craxii-server | wc -w)" -eq 1 ]] || fail "craxii-server has supplementary groups"
[[ "$(id -G craxii | wc -w)" -eq 1 ]] || fail "craxii has supplementary groups"
[[ "$(passwd -S craxii-server | awk '{print $2}')" == L ]] || fail "craxii-server is not locked"
[[ "$(passwd -S craxii | awk '{print $2}')" == L ]] || fail "craxii is not locked"

install -d -o root -g root -m 0755 /opt/craxii /opt/craxii/releases
release_directory="/opt/craxii/releases/${release_version}"
if [[ -e "${release_directory}" ]]; then
  echo "error: immutable release already exists: ${release_directory}" >&2
  exit 1
fi
install -d -o root -g root -m 0755 "${release_directory}"
install -o root -g root -m 0555 "${source_directory}/craxii-server" "${release_directory}/craxii-server"
install -o root -g root -m 0555 "${source_directory}/craxii-admin" "${release_directory}/craxii-admin"
install -o root -g root -m 0555 "${source_directory}/craxii-workstation-reader" "${release_directory}/craxii-workstation-reader"
install -o root -g craxii-server -m 4750 "${source_directory}/craxii-workstation-launcher" "${release_directory}/craxii-workstation-launcher"

for binary in craxii-server craxii-admin craxii-workstation-launcher craxii-workstation-reader; do
  sha256sum "${release_directory}/${binary}"
done
temporary_link="/opt/craxii/.current-${release_version}-$$"
ln -s "releases/${release_version}" "${temporary_link}"
mv -Tf "${temporary_link}" /opt/craxii/current

install -d -o root -g craxii-server -m 0750 /etc/craxii
install -d -o craxii-server -g craxii-server -m 0700 /etc/craxii/credentials
install -o root -g craxii-server -m 0640 "${asset_directory}/config.toml.template" /etc/craxii/config.toml
install -o root -g root -m 0644 "${asset_directory}/craxii-server.service" /etc/systemd/system/craxii-server.service

install -d -o craxii-server -g craxii-server -m 0700 /var/lib/craxii
install -d -o craxii-server -g craxii-server -m 0700 /var/lib/craxii/db
install -d -o craxii-server -g craxii-server -m 0700 /var/lib/craxii/artifacts
install -d -o craxii-server -g craxii-server -m 0700 /var/lib/craxii/locks
install -d -o craxii-server -g craxii-server -m 0700 /var/cache/craxii
install -d -o craxii-server -g craxii-server -m 0700 /run/craxii

install -d -o root -g root -m 0755 /srv/craxii /srv/craxii/workspaces
install -d -o craxii -g craxii -m 0700 /srv/craxii/workspaces/primary
setfacl -m u::rwx,u:craxii-server:r-x,g::---,m::r-x,o::--- /srv/craxii/workspaces/primary
setfacl -m d:u::rwx,d:u:craxii-server:r-x,d:g::---,d:m::r-x,d:o::--- /srv/craxii/workspaces/primary
chown craxii:craxii /home/craxii
chmod 0700 /home/craxii

systemctl daemon-reload
systemd-analyze verify /etc/systemd/system/craxii-server.service
if systemctl is-active --quiet craxii-server.service; then
  fail "craxii-server.service became active during bootstrap"
fi
if systemctl is-enabled --quiet craxii-server.service; then
  fail "craxii-server.service became enabled during bootstrap"
fi

echo "Stage 27 security-boundary assets installed; service remains stopped and no credential was created."
