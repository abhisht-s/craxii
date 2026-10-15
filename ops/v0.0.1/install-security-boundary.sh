#!/bin/sh
# Image/isolated guest installation only. No cloud action and no service activation.
set -eu
umask 077
fail() { printf '%s\n' isolation_unavailable >&2; exit 1; }
[ "$(id -u)" = 0 ] || fail
[ "$(uname -m)" = x86_64 ] || fail
[ "$#" = 2 ] || fail
case "$2" in *[!a-f0-9]*|'') fail ;; esac
[ "${#2}" = 64 ] || fail
/usr/bin/grep -qx 'ID=ubuntu' /etc/os-release || fail
/usr/bin/grep -qx 'VERSION_ID="24.04"' /etc/os-release || fail
[ -f /sys/fs/cgroup/cgroup.controllers ] || fail
assets=$(CDPATH='' cd -- "$(dirname -- "$0")" && pwd)
release_source=$1
release_id=$2
# Every source artifact must be an ordinary single-link binary; integrity is checked after copying.
for binary in craxii-server craxii-guest-guard craxii-workstation-launcher craxii-workstation-reader; do
  [ -f "$release_source/$binary" ] && [ ! -L "$release_source/$binary" ] || fail
done
for unit in craxii-server.service craxii-distribution.service; do
  if /usr/bin/systemctl is-active --quiet "$unit" || /usr/bin/systemctl is-enabled --quiet "$unit"; then fail; fi
done
for group in craxii-server craxii craxii-observe; do
  /usr/bin/getent group "$group" >/dev/null || /usr/sbin/groupadd --system "$group"
done
/usr/bin/getent passwd craxii-server >/dev/null || /usr/sbin/useradd --system --gid craxii-server --home-dir /var/lib/craxii --no-create-home --shell /usr/sbin/nologin craxii-server
/usr/bin/getent passwd craxii >/dev/null || /usr/sbin/useradd --system --gid craxii --home-dir /home/craxii --no-create-home --shell /bin/bash craxii
/usr/sbin/usermod -G craxii-observe craxii-server
/usr/sbin/usermod -G '' craxii
/usr/bin/passwd -l craxii-server >/dev/null
/usr/bin/passwd -l craxii >/dev/null
[ "$(id -u craxii)" != "$(id -u craxii-server)" ] || fail
[ "$(id -u craxii)" != 0 ] && [ "$(id -u craxii-server)" != 0 ] || fail
[ "$(id -G craxii | wc -w)" -eq 1 ] || fail
/usr/bin/install -d -o root -g root -m 0755 /opt/craxii /opt/craxii/releases
[ ! -e "/opt/craxii/releases/$release_id" ] || fail
/usr/bin/install -d -o root -g root -m 0755 "/opt/craxii/releases/$release_id"
/usr/bin/install -o root -g craxii-server -m 0550 "$release_source/craxii-server" "/opt/craxii/releases/$release_id/craxii-server"
/usr/bin/install -o root -g craxii-server -m 0550 "$release_source/craxii-guest-guard" "/opt/craxii/releases/$release_id/craxii-guest-guard"
/usr/bin/install -o root -g craxii-server -m 4750 "$release_source/craxii-workstation-launcher" "/opt/craxii/releases/$release_id/craxii-workstation-launcher"
/usr/bin/install -o root -g root -m 0111 "$release_source/craxii-workstation-reader" "/opt/craxii/releases/$release_id/craxii-workstation-reader"
actual=$(/usr/bin/sha256sum "/opt/craxii/releases/$release_id/craxii-server")
[ "${actual%% *}" = "$release_id" ] || fail
[ ! -e /opt/craxii/current ] && [ ! -L /opt/craxii/current ] || fail
/bin/ln -s "releases/$release_id" /opt/craxii/current
/usr/bin/install -d -o root -g root -m 0755 /opt/craxii/guest
for asset in bootstrap-data-volume.py security-boundary.py guest-bootstrap.schema.json; do
  /usr/bin/install -o root -g root -m 0555 "$assets/$asset" "/opt/craxii/guest/$asset"
done
/usr/bin/install -d -o root -g craxii-server -m 0750 /etc/craxii /var/lib/craxii-worker /run/craxii-worker
/usr/bin/install -d -o root -g root -m 0700 /var/lib/craxii-worker/bootstrap
/usr/bin/install -d -o craxii-server -g craxii-server -m 0700 /var/lib/craxii-worker/identity
# Empty root-owned fallback targets only; preserve any existing content by refusing.
for target in /srv/craxii-data /var/lib/craxii /srv/craxii/workspaces /home/craxii; do
  [ ! -L "$target" ] || fail
  if [ -d "$target" ]; then [ -z "$(/usr/bin/find "$target" -mindepth 1 -maxdepth 1 -print -quit)" ] || fail; fi
  /usr/bin/install -d -o root -g root -m 0755 "$target"
done
/usr/bin/install -o root -g root -m 0644 "$assets/craxii-guest-bootstrap.service" /etc/systemd/system/craxii-guest-bootstrap.service
/usr/bin/install -o root -g root -m 0644 "$assets/craxii-distribution.service" /etc/systemd/system/craxii-distribution.service
/usr/bin/install -o root -g root -m 0644 "$assets/craxii-guest-network-check.service" /etc/systemd/system/craxii-guest-network-check.service
/usr/bin/install -o root -g root -m 0644 "$assets/sysctl.conf" /etc/sysctl.d/90-craxii-guest.conf
/usr/sbin/sysctl --system >/dev/null 2>&1 || fail
observe_gid=$(/usr/bin/getent group craxii-observe | cut -d: -f3)
/usr/bin/mount -o "remount,hidepid=2,gid=$observe_gid" /proc
# System-level persistence paths cannot escape the tool cgroup.
/usr/bin/systemctl mask cron.service atd.service "user@$(id -u craxii).service" >/dev/null 2>&1
[ ! -e /var/lib/systemd/linger/craxii ] || fail
for file in /etc/cron.allow /etc/at.allow; do
  [ ! -e "$file" ] || fail
  /usr/bin/install -o root -g root -m 0600 /dev/null "$file"
done
for directory in /run/dbus /run/lock /var/crash; do
  if [ -d "$directory" ]; then /bin/chmod 0700 "$directory"; fi
done
# Deny model execution of privilege/persistence helpers in addition to account/group restrictions.
for binary in sudo su pkexec systemctl busctl crontab at newuidmap newgidmap mount umount; do
  location=$(command -v "$binary" || true)
  if [ -n "$location" ]; then /usr/bin/setfacl -m u:craxii:--- "$location"; fi
done
/usr/bin/systemctl daemon-reload
/usr/bin/systemd-analyze verify /etc/systemd/system/craxii-distribution.service /etc/systemd/system/craxii-guest-bootstrap.service /etc/systemd/system/craxii-guest-network-check.service >/dev/null 2>&1 || fail
# No service is started/enabled. Per-installation config, quotas and firewall must be prepared and
# rechecked by the root pre-start gate. Incomplete image policy remains unavailable.
printf '%s\n' guest_assets_installed_unready
