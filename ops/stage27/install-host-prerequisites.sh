#!/usr/bin/env bash
set -euo pipefail

fail() {
  echo "error: $*" >&2
  exit 1
}

(( EUID == 0 )) || fail "run as root on the explicitly authorized Stage 27 host"
[[ "$(uname -m)" == x86_64 ]] || fail "expected x86_64 host"
grep -qx 'ID=ubuntu' /etc/os-release || fail "expected Ubuntu"
grep -qx 'VERSION_ID="24.04"' /etc/os-release || fail "expected Ubuntu 24.04"

# Fresh Ubuntu 24.04 EC2 images can name official mirrors with an AWS-region prefix. Convert only
# official Ubuntu archive/security URLs; do not add a mirror or broaden network access.
mapfile -t apt_sources < <(
  find /etc/apt -maxdepth 2 -type f \( -name '*.list' -o -name '*.sources' \) -print
)
(( ${#apt_sources[@]} > 0 )) || fail "no APT source files found"
for source in "${apt_sources[@]}"; do
  sed -E -i \
    -e 's|http://(([A-Za-z0-9-]+\.)*archive\.ubuntu\.com/ubuntu)|https://\1|g' \
    -e 's|http://security\.ubuntu\.com/ubuntu|https://security.ubuntu.com/ubuntu|g' \
    "${source}"
done
if grep -ERq 'http://(([A-Za-z0-9-]+\.)*archive\.ubuntu\.com/ubuntu|security\.ubuntu\.com/ubuntu)' \
  "${apt_sources[@]}"; then
  fail "an official Ubuntu APT source still uses HTTP"
fi

export DEBIAN_FRONTEND=noninteractive
apt-get update
apt-get install -y --no-install-recommends \
  acl \
  bash \
  build-essential \
  ca-certificates \
  curl \
  e2fsprogs \
  git \
  procps \
  sqlite3 \
  util-linux

if ! getent group craxii-build >/dev/null; then
  groupadd --system craxii-build
fi
if ! getent passwd craxii-build >/dev/null; then
  useradd --system --gid craxii-build --home-dir /var/lib/craxii-build \
    --create-home --shell /usr/sbin/nologin craxii-build
fi
[[ "$(id -gn craxii-build)" == craxii-build ]] || fail "unexpected craxii-build primary group"
[[ "$(getent passwd craxii-build | cut -d: -f6-7)" == "/var/lib/craxii-build:/usr/sbin/nologin" ]] ||
  fail "unexpected craxii-build account"
usermod -G '' craxii-build
passwd -l craxii-build >/dev/null
install -d -o craxii-build -g craxii-build -m 0700 \
  /var/lib/craxii-build \
  /var/lib/craxii-build/cargo \
  /var/lib/craxii-build/rustup \
  /var/lib/craxii-build/target

rustup_init=/var/lib/craxii-build/rustup-init
if [[ ! -x /var/lib/craxii-build/cargo/bin/rustup ]]; then
  curl --proto '=https' --tlsv1.2 --fail --silent --show-error \
    https://sh.rustup.rs -o "${rustup_init}.sh"
  chown craxii-build:craxii-build "${rustup_init}.sh"
  chmod 0500 "${rustup_init}.sh"
  runuser -u craxii-build -- /usr/bin/env -i \
    HOME=/var/lib/craxii-build \
    CARGO_HOME=/var/lib/craxii-build/cargo \
    RUSTUP_HOME=/var/lib/craxii-build/rustup \
    PATH=/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin \
    /bin/sh "${rustup_init}.sh" -y --no-modify-path --profile minimal \
      --default-toolchain 1.98.0 \
      --target x86_64-unknown-linux-gnu \
      --component rustfmt \
      --component clippy
  rm -f "${rustup_init}.sh"
else
  runuser -u craxii-build -- /usr/bin/env -i \
    HOME=/var/lib/craxii-build \
    CARGO_HOME=/var/lib/craxii-build/cargo \
    RUSTUP_HOME=/var/lib/craxii-build/rustup \
    PATH=/var/lib/craxii-build/cargo/bin:/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin \
    /var/lib/craxii-build/cargo/bin/rustup toolchain install 1.98.0 \
      --profile minimal \
      --target x86_64-unknown-linux-gnu \
      --component rustfmt \
      --component clippy
fi

rust_version="$(runuser -u craxii-build -- /usr/bin/env -i \
  RUSTUP_HOME=/var/lib/craxii-build/rustup \
  CARGO_HOME=/var/lib/craxii-build/cargo \
  /var/lib/craxii-build/cargo/bin/rustc +1.98.0 --version)"
[[ "${rust_version}" == 'rustc 1.98.0 '* ]] || fail "Rust 1.98.0 was not installed"
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
echo "Stage 27 host packages and controlled Rust build toolchain installed: ${rust_version}"
