#!/usr/bin/env bash
set -euo pipefail

readonly repository=https://github.com/abhisht-s/craxii.git
readonly source_directory=/var/lib/craxii-build/source
readonly target_directory=/var/lib/craxii-build/target
readonly cargo_home=/var/lib/craxii-build/cargo
readonly rustup_home=/var/lib/craxii-build/rustup

fail() {
  echo "error: $*" >&2
  exit 1
}

(( EUID == 0 )) || fail "run as root on the explicitly authorized Stage 27 host"
[[ $# -eq 1 && "$1" =~ ^[0-9a-f]{40}$ ]] || fail "usage: $0 <exact-40-character-commit>"
commit="$1"
getent passwd craxii-build >/dev/null || fail "craxii-build account is absent"
[[ -x "${cargo_home}/bin/cargo" ]] || fail "controlled Rust toolchain is absent"

build_git() {
  runuser -u craxii-build -- /usr/bin/env -i \
    HOME=/var/lib/craxii-build \
    PATH=/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin \
    git -C "${source_directory}" "$@"
}

if [[ ! -d "${source_directory}/.git" ]]; then
  [[ ! -e "${source_directory}" ]] || fail "build source path exists but is not a Git checkout"
  runuser -u craxii-build -- /usr/bin/env -i \
    HOME=/var/lib/craxii-build \
    PATH=/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin \
    git clone "${repository}" "${source_directory}"
fi

[[ "$(build_git remote get-url origin)" == "${repository}" ]] ||
  fail "build checkout has an unexpected origin"
[[ -z "$(build_git status --porcelain=v1 --untracked-files=normal)" ]] ||
  fail "build checkout is dirty"
build_git fetch --force --no-tags origin \
  +refs/heads/main:refs/remotes/origin/main
build_git merge-base --is-ancestor "${commit}" origin/main ||
  fail "requested commit is not reachable from GitHub origin/main"
build_git checkout --detach "${commit}"
[[ "$(build_git rev-parse HEAD)" == "${commit}" ]] ||
  fail "checkout did not resolve to the requested commit"
[[ -z "$(build_git status --porcelain=v1 --untracked-files=normal)" ]] ||
  fail "exact checkout is not clean"
[[ "$(<"${source_directory}/rust-toolchain.toml")" == *'channel = "1.98.0"'* ]] ||
  fail "repository toolchain pin is not Rust 1.98.0"

runuser -u craxii-build -- /usr/bin/env -i \
  HOME=/var/lib/craxii-build \
  CARGO_HOME="${cargo_home}" \
  RUSTUP_HOME="${rustup_home}" \
  CARGO_TARGET_DIR="${target_directory}" \
  CARGO_BUILD_JOBS=2 \
  PATH=${cargo_home}/bin:/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin \
  cargo +1.98.0 build --locked --release -p craxii-server \
    --bin craxii-server \
    --bin craxii-admin \
    --bin craxii-workstation-launcher \
    --bin craxii-workstation-reader \
    --manifest-path "${source_directory}/Cargo.toml"

for binary in craxii-server craxii-admin craxii-workstation-launcher craxii-workstation-reader; do
  [[ -f "${target_directory}/release/${binary}" && -x "${target_directory}/release/${binary}" ]] ||
    fail "release binary is absent: ${binary}"
done
echo "Stage 27 immutable source checkout and release build passed for ${commit}."
printf 'STAGE27_RELEASE_SOURCE=%s\n' "${target_directory}/release"
printf 'STAGE27_RELEASE_VERSION=0.0.1-%s\n' "${commit:0:12}"
