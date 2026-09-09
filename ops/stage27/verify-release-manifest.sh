#!/usr/bin/env bash
set -euo pipefail

fail() {
  echo "error: $*" >&2
  exit 1
}

[[ $# -eq 2 ]] || fail "usage: $0 <built-release-directory> <expected-commit-or-prefix>"
readonly source_directory="$1"
readonly expected_revision="$2"
readonly manifest="${source_directory}/.craxii-stage27-build-manifest"
readonly -a binaries=(
  craxii-server
  craxii-admin
  craxii-stage27-luna-benchmark
  craxii-workstation-launcher
  craxii-workstation-reader
)

[[ "${expected_revision}" =~ ^[0-9a-f]{12}([0-9a-f]{28})?$ ]] ||
  fail "expected revision must be an exact 12- or 40-character lowercase Git revision"
[[ -f "${manifest}" && ! -L "${manifest}" ]] || fail "trusted build manifest is absent or unsafe"
[[ "$(wc -l <"${manifest}")" -eq 6 ]] || fail "trusted build manifest has unexpected entries"
IFS= read -r commit_line <"${manifest}"
[[ "${commit_line}" =~ ^commit=([0-9a-f]{40})$ ]] || fail "trusted build manifest commit is malformed"
readonly manifest_commit="${BASH_REMATCH[1]}"
[[ "${manifest_commit:0:${#expected_revision}}" == "${expected_revision}" ]] ||
  fail "trusted build manifest does not match the requested revision"

for binary in "${binaries[@]}"; do
  [[ "$(grep -Ec "^[0-9a-f]{64}  ${binary}$" "${manifest}")" -eq 1 ]] ||
    fail "trusted build manifest entry mismatch: ${binary}"
done
(
  cd "${source_directory}"
  tail -n +2 "${manifest}" | sha256sum --check --strict - >/dev/null
) || fail "built release differs from its trusted build manifest"

printf 'STAGE27_VERIFIED_BUILD_COMMIT=%s\n' "${manifest_commit}"
