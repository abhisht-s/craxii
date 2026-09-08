#!/usr/bin/env bash
set -euo pipefail

readonly expected_size_bytes=85899345920
readonly data_mount=/srv/craxii-data
readonly state_mount=/var/lib/craxii
readonly workspaces_mount=/srv/craxii/workspaces
readonly home_mount=/home/craxii
readonly marker_name=.craxii-stage27-data-volume
readonly confirmation=--initialize-exactly-one-unformatted-80-gib-ebs-volume

fail() {
  echo "error: $*" >&2
  exit 1
}

require_root_and_tools() {
  (( EUID == 0 )) || fail "run as root on the explicitly authorized Stage 27 host"
  [[ "$(uname -m)" == x86_64 ]] || fail "expected x86_64 host"
  grep -qx 'ID=ubuntu' /etc/os-release || fail "expected Ubuntu"
  grep -qx 'VERSION_ID="24.04"' /etc/os-release || fail "expected Ubuntu 24.04"
  local command
  for command in awk blkid find findmnt grep install lsblk mkfs.ext4 mount mountpoint \
    stat systemctl wipefs; do
    command -v "${command}" >/dev/null || fail "missing required command: ${command}"
  done
}

fstab_has_target() {
  local target="$1"
  awk -v target="${target}" '$1 !~ /^#/ && $2 == target { found = 1 } END { exit !found }' /etc/fstab
}

assert_fstab_line() {
  local source="$1"
  local target="$2"
  local filesystem="$3"
  local options="$4"
  awk -v source="${source}" -v target="${target}" -v filesystem="${filesystem}" \
    -v options="${options}" \
    '$1 == source && $2 == target && $3 == filesystem && $4 == options { found = 1 } \
     END { exit !found }' /etc/fstab || fail "missing or altered fstab entry for ${target}"
}

assert_bind_mount() {
  local target="$1"
  local expected_root="$2"
  mountpoint -q "${target}" || fail "required bind mount is absent: ${target}"
  [[ "$(findmnt -nro FSROOT --target "${target}")" == "${expected_root}" ]] ||
    fail "unexpected bind source for ${target}"
}

verify_layout() {
  mountpoint -q "${data_mount}" || fail "${data_mount} is not mounted"
  [[ "$(findmnt -nro FSTYPE --target "${data_mount}")" == ext4 ]] ||
    fail "${data_mount} is not ext4"

  local source uuid marker
  source="$(findmnt -nro SOURCE --target "${data_mount}")"
  source="${source%%\[*}"
  uuid="$(blkid -s UUID -o value "${source}")"
  [[ -n "${uuid}" ]] || fail "data filesystem has no UUID"
  marker="${data_mount}/${marker_name}"
  [[ -f "${marker}" && ! -L "${marker}" ]] || fail "Craxii data-volume marker is absent"
  [[ "$(<"${marker}")" == "${uuid}" ]] || fail "Craxii data-volume marker UUID mismatch"
  [[ "$(stat -c '%U:%G:%a' "${marker}")" == root:root:400 ]] ||
    fail "Craxii data-volume marker metadata mismatch"

  assert_fstab_line "UUID=${uuid}" "${data_mount}" ext4 \
    defaults,nofail,x-systemd.device-timeout=30s
  assert_fstab_line "${data_mount}/state" "${state_mount}" none \
    bind,nofail,x-systemd.requires-mounts-for=/srv/craxii-data
  assert_fstab_line "${data_mount}/workspaces" "${workspaces_mount}" none \
    bind,nofail,x-systemd.requires-mounts-for=/srv/craxii-data
  assert_fstab_line "${data_mount}/home/craxii" "${home_mount}" none \
    bind,nofail,x-systemd.requires-mounts-for=/srv/craxii-data
  assert_bind_mount "${state_mount}" /state
  assert_bind_mount "${workspaces_mount}" /workspaces
  assert_bind_mount "${home_mount}" /home/craxii
  findmnt --verify --tab-file /etc/fstab >/dev/null
  echo "Stage 27 data volume and persistent bind mounts verified (UUID=${uuid})."
}

device_has_mount_or_swap() {
  local device="$1"
  lsblk -nrpo MOUNTPOINTS "${device}" | grep -q '[^[:space:]]'
}

device_has_children_or_holders() {
  local device="$1"
  local -a nodes holders
  mapfile -t nodes < <(lsblk -nrpo PATH "${device}")
  mapfile -t holders < <(find "/sys/class/block/$(basename "${device}")/holders" \
    -mindepth 1 -maxdepth 1 -print 2>/dev/null)
  (( ${#nodes[@]} != 1 || ${#holders[@]} != 0 ))
}

device_has_signature() {
  local device="$1"
  [[ -n "$(wipefs -n "${device}" 2>/dev/null)" ]] || blkid -p "${device}" >/dev/null 2>&1
}

require_root_and_tools

case "${1:-}" in
  --verify-only)
    verify_layout
    exit 0
    ;;
  "${confirmation}" | "") ;;
  *) fail "usage: $0 [--verify-only|${confirmation}]" ;;
esac

if mountpoint -q "${data_mount}"; then
  verify_layout
  exit 0
fi

if fstab_has_target "${data_mount}"; then
  [[ ! -e "${data_mount}/${marker_name}" ]] || fail "data marker is visible without its mount"
  mount "${data_mount}" || fail "existing UUID-based data mount could not be mounted"
  mount "${state_mount}" || fail "existing state bind mount could not be mounted"
  mount "${workspaces_mount}" || fail "existing workspace bind mount could not be mounted"
  mount "${home_mount}" || fail "existing home bind mount could not be mounted"
  verify_layout
  exit 0
fi

for target in "${state_mount}" "${workspaces_mount}" "${home_mount}"; do
  ! fstab_has_target "${target}" || fail "partial/conflicting fstab entry exists for ${target}"
done
[[ "${1:-}" == "${confirmation}" ]] || fail \
  "initialization requires the explicit confirmation argument: ${confirmation}"

for target in "${data_mount}" "${state_mount}" "${workspaces_mount}" "${home_mount}"; do
  [[ ! -L "${target}" ]] || fail "mount target is a symbolic link: ${target}"
  if [[ -d "${target}" && -n "$(find "${target}" -mindepth 1 -maxdepth 1 -print -quit)" ]]; then
    fail "refusing to hide existing content under mount target ${target}"
  fi
done

mapfile -t size_matches < <(
  lsblk -bdnro PATH,TYPE,SIZE | awk -v size="${expected_size_bytes}" \
    '$2 == "disk" && $3 == size { print $1 }'
)
(( ${#size_matches[@]} > 0 )) || fail "no whole 80 GiB disk was found"

candidates=()
rejected=()
for device in "${size_matches[@]}"; do
  if device_has_mount_or_swap "${device}"; then
    rejected+=("${device}: mounted or swap-backed")
  elif device_has_children_or_holders "${device}"; then
    rejected+=("${device}: has partitions or holders")
  elif device_has_signature "${device}"; then
    rejected+=("${device}: contains an existing signature")
  else
    candidates+=("${device}")
  fi
done

printf '80 GiB disk candidates considered:\n'
printf '  %s\n' "${size_matches[@]}"
if (( ${#rejected[@]} > 0 )); then
  printf 'Rejected candidates:\n'
  printf '  %s\n' "${rejected[@]}"
fi
(( ${#candidates[@]} == 1 )) || fail \
  "expected exactly one unformatted, unmounted, whole 80 GiB disk; found ${#candidates[@]}"
device="${candidates[0]}"

# Repeat every destructive precondition immediately before mkfs. Never format by a caller-supplied
# device name, an NVMe ordinal, or an ambiguous size match.
[[ "$(lsblk -bdnro TYPE,SIZE "${device}")" == "disk ${expected_size_bytes}" ]] ||
  fail "candidate identity changed before formatting"
! device_has_mount_or_swap "${device}" || fail "candidate became mounted or swap-backed"
! device_has_children_or_holders "${device}" || fail "candidate gained partitions or holders"
! device_has_signature "${device}" || fail "candidate gained a filesystem/device signature"

echo "Initializing the sole safe candidate: ${device}"
if command -v nvme >/dev/null && [[ "${device}" == /dev/nvme* ]]; then
  nvme id-ctrl "${device}" 2>/dev/null | sed -n '/^sn[[:space:]]*:/p;/^mn[[:space:]]*:/p' || true
elif [[ -r "/sys/class/block/$(basename "${device}")/device/serial" ]]; then
  printf 'serial: %s\n' "$(<"/sys/class/block/$(basename "${device}")/device/serial")"
fi
mkfs.ext4 -F -L craxii-data "${device}"
uuid="$(blkid -s UUID -o value "${device}")"
[[ -n "${uuid}" ]] || fail "new ext4 filesystem has no UUID"

install -d -o root -g root -m 0755 \
  "${data_mount}" "${state_mount}" /srv/craxii "${workspaces_mount}" /home "${home_mount}"
mount -t ext4 -o defaults "UUID=${uuid}" "${data_mount}"
install -d -o root -g root -m 0755 \
  "${data_mount}/state" "${data_mount}/workspaces" "${data_mount}/home" \
  "${data_mount}/home/craxii"
printf '%s\n' "${uuid}" >"${data_mount}/${marker_name}"
chown root:root "${data_mount}/${marker_name}"
chmod 0400 "${data_mount}/${marker_name}"

for target in "${state_mount}" "${workspaces_mount}" "${home_mount}"; do
  [[ -z "$(find "${target}" -mindepth 1 -maxdepth 1 -print -quit)" ]] ||
    fail "refusing to hide existing content under bind mount target ${target}"
done

fstab_candidate="$(mktemp /etc/fstab.craxii-stage27.XXXXXX)"
cleanup_fstab_candidate() { rm -f "${fstab_candidate}"; }
trap cleanup_fstab_candidate EXIT
cp --preserve=mode,ownership /etc/fstab "${fstab_candidate}"
{
  printf '\n# Craxii Stage 27 persistent data layout\n'
  printf 'UUID=%s %s ext4 defaults,nofail,x-systemd.device-timeout=30s 0 2\n' \
    "${uuid}" "${data_mount}"
  printf '%s/state %s none bind,nofail,x-systemd.requires-mounts-for=/srv/craxii-data 0 0\n' \
    "${data_mount}" "${state_mount}"
  printf '%s/workspaces %s none bind,nofail,x-systemd.requires-mounts-for=/srv/craxii-data 0 0\n' \
    "${data_mount}" "${workspaces_mount}"
  printf '%s/home/craxii %s none bind,nofail,x-systemd.requires-mounts-for=/srv/craxii-data 0 0\n' \
    "${data_mount}" "${home_mount}"
} >>"${fstab_candidate}"
findmnt --verify --tab-file "${fstab_candidate}" >/dev/null
install -o root -g root -m 0644 "${fstab_candidate}" /etc/fstab
trap - EXIT
rm -f "${fstab_candidate}"

systemctl daemon-reload
mount "${state_mount}"
mount "${workspaces_mount}"
mount "${home_mount}"
verify_layout
