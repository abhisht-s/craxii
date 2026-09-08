#!/usr/bin/env bash
set -euo pipefail

readonly credential_directory=/etc/craxii/credentials
readonly credential_path=${credential_directory}/openai_provider

fail() {
  echo "error: $*" >&2
  exit 1
}

(( EUID == 0 )) || fail "run as root on the explicitly authorized Stage 27 host"
[[ -t 0 ]] || fail "credential installation requires an interactive terminal"
[[ "$(stat -c '%U:%G:%a' "${credential_directory}")" == craxii-server:craxii-server:700 ]] ||
  fail "credential directory metadata is not the Stage 27 contract"
[[ ! -e "${credential_path}" && ! -L "${credential_path}" ]] ||
  fail "provider credential already exists; this command never overwrites it"

IFS= read -r -s -p 'OpenAI API key (input hidden): ' provider_key
printf '\n'
[[ -n "${provider_key}" ]] || fail "empty credential refused"
[[ "${provider_key}" != *[[:space:]]* ]] || fail "credential containing whitespace refused"
umask 077
printf '%s' "${provider_key}" | install -o craxii-server -g craxii-server -m 0600 \
  /dev/stdin "${credential_path}"
unset provider_key
[[ "$(stat -c '%U:%G:%a:%h' "${credential_path}")" == craxii-server:craxii-server:600:1 ]] ||
  fail "credential file metadata verification failed"
echo "Provider credential installed with verified metadata; contents were not printed."
