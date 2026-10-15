#!/bin/sh
# Capability preflight only; no mount, cgroup remount or destructive fallback.
set -eu
[ "$(cat /etc/craxii-v001-fixture 2>/dev/null)" = craxii-v001-guest-boundary-disposable ]
[ "$(cat /sys/class/dmi/id/sys_vendor)" = QEMU ]
[ "$(cat /sys/class/dmi/id/product_uuid)" = "$(cat /etc/craxii-v001-fixture-instance)" ]
[ "$(id -u)" = 0 ]
[ -f /sys/fs/cgroup/cgroup.controllers ]
for tool in python3 losetup mkfs.ext4 mount setquota quotaon nft ip systemd-run; do command -v "$tool" >/dev/null; done
