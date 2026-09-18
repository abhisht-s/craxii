#!/usr/bin/env bash
set -euo pipefail
PATH=/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin
LC_ALL=C
LANG=C
export PATH
export LC_ALL LANG
readonly PATH LC_ALL LANG
umask 077

fail() {
  echo "error: $*" >&2
  exit 1
}

usage() {
  echo "usage: $0 (--pre-reboot|--post-reboot) <deployed-40-character-commit> <data-filesystem-uuid>" >&2
  exit 2
}

[[ $# -eq 3 ]] || usage
readonly mode="$1"
readonly deployment_commit="$2"
readonly data_uuid="$3"
[[ "${mode}" == --pre-reboot || "${mode}" == --post-reboot ]] || usage
[[ "${deployment_commit}" =~ ^[0-9a-f]{40}$ ]] || usage
[[ "${data_uuid}" =~ ^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$ ]] || usage
(( EUID == 0 )) || fail "run as root inside the authorized Stage 27 Session Manager host"
[[ "$(uname -s)" == Linux && "$(uname -m)" == x86_64 ]] || fail "expected Linux x86-64 host"
grep -qx 'ID=ubuntu' /etc/os-release || fail "expected Ubuntu"
grep -qx 'VERSION_ID="24.04"' /etc/os-release || fail "expected Ubuntu 24.04"

asset_directory="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
readonly asset_directory
readonly evidence_helper="${asset_directory}/production-evidence.py"
readonly python=/usr/bin/python3
readonly checkout=/var/lib/craxii-build/source
readonly cargo=/var/lib/craxii-build/cargo/bin/cargo
readonly target_directory=/var/lib/craxii-build/target
readonly service=craxii-server.service
readonly launcher=/opt/craxii/current/craxii-workstation-launcher
readonly cgroup_root=/sys/fs/cgroup/system.slice/craxii-server.service/craxii-executions
readonly workspace_sentinel=/srv/craxii/workspaces/primary/.craxii-stage27-persistence-sentinel-v1
readonly evidence_directory=/srv/craxii-data/stage27-evidence
readonly evidence_sentinel=${evidence_directory}/persistence-sentinel-v1
readonly pre_reboot_evidence=${evidence_directory}/pre-reboot-v1.json
readonly restart_comparison=${evidence_directory}/service-restart-comparison-v1.json
readonly post_reboot_evidence=${evidence_directory}/post-reboot-v1.json
readonly reboot_comparison=${evidence_directory}/reboot-comparison-v1.json
runtime_directory="/run/craxii-stage27-gate-$$"
readonly runtime_directory
control_pid=""
live_test_pid=""
declare -a check_failures=()
last_check_name=""
last_check_status=""

cleanup() {
  if [[ -n "${live_test_pid}" ]]; then
    kill "${live_test_pid}" 2>/dev/null || true
    wait "${live_test_pid}" 2>/dev/null || true
  fi
  if [[ -n "${control_pid}" ]]; then
    kill "${control_pid}" 2>/dev/null || true
    wait "${control_pid}" 2>/dev/null || true
  fi
  if [[ "${runtime_directory}" == /run/craxii-stage27-gate-[0-9]* ]]; then
    rm -rf -- "${runtime_directory}"
  fi
}
trap cleanup EXIT

[[ -f "${evidence_helper}" && ! -L "${evidence_helper}" ]] ||
  fail "Stage 27 evidence helper is absent or unsafe"
[[ -x "${python}" ]] || fail "system Python is absent"
[[ -f "${asset_directory}/bootstrap-data-volume.sh" &&
   ! -L "${asset_directory}/bootstrap-data-volume.sh" ]] || fail "data-volume verifier is absent or unsafe"
install -d -o craxii-server -g craxii-server -m 0700 "${runtime_directory}"

prepare_evidence_directory() {
  if [[ -e "${evidence_directory}" || -L "${evidence_directory}" ]]; then
    [[ -d "${evidence_directory}" && ! -L "${evidence_directory}" ]] ||
      fail "persistent evidence directory is unsafe"
    [[ "$(stat -c '%U:%G:%a' "${evidence_directory}")" == root:root:700 ]] ||
      fail "persistent evidence directory metadata mismatch"
  else
    install -d -o root -g root -m 0700 "${evidence_directory}"
  fi
}

build_git() {
  runuser -u craxii-build -- /usr/bin/env -i \
    HOME=/var/lib/craxii-build \
    PATH=/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin \
    /usr/bin/git -C "${checkout}" "$@"
}

run_build() {
  (( $# > 0 )) || fail "Cargo subcommand is required"
  local cargo_subcommand="$1"
  shift
  runuser -u craxii-build -- /usr/bin/env -i \
    HOME=/var/lib/craxii-build \
    CARGO_HOME=/var/lib/craxii-build/cargo \
    RUSTUP_HOME=/var/lib/craxii-build/rustup \
    CARGO_TARGET_DIR="${target_directory}" \
    CARGO_BUILD_JOBS=2 \
    PATH=/var/lib/craxii-build/cargo/bin:/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin \
    "${cargo}" +1.98.0 "${cargo_subcommand}" \
    --manifest-path "${checkout}/Cargo.toml" "$@"
}

wait_for_path() {
  local path="$1"
  for _ in {1..600}; do
    [[ -e "${path}" ]] && return 0
    sleep 0.1
  done
  fail "timed out waiting for disposable verification marker"
}

wait_ready() {
  for _ in {1..600}; do
    if [[ "$(curl --silent --output /dev/null --write-out '%{http_code}' \
      --connect-timeout 1 --max-time 2 http://127.0.0.1:8080/health/ready || true)" == 200 ]]; then
      return 0
    fi
    sleep 0.1
  done
  fail "backend did not return to ready state"
}

snapshot() {
  local phase="$1"
  local output="$2"
  shift 2
  "${python}" "${evidence_helper}" snapshot \
    --phase "${phase}" \
    --deployment-commit "${deployment_commit}" \
    --data-uuid "${data_uuid}" \
    --output "${output}" \
    "$@"
}

run_independent_check() {
  if (( $# < 2 )); then
    printf 'error: independent check requires a name and command\n' >&2
    return 2
  fi
  local name="$1"
  shift
  local status
  if [[ ! "${name}" =~ ^[a-z0-9][a-z0-9-]*$ ]]; then
    printf 'error: invalid independent check name: %s\n' "${name:-<empty>}" >&2
    return 2
  fi
  printf 'STAGE27_CHECK_START=%s\n' "${name}"
  set +e
  (
    set -e
    "$@"
  )
  status=$?
  set -e
  last_check_name="${name}"
  last_check_status="${status}"
  if [[ "${status}" -eq 0 ]]; then
    printf 'STAGE27_CHECK=%s:PASS\n' "${name}"
  else
    printf 'STAGE27_CHECK=%s:FAIL:status=%s\n' "${name}" "${status}" >&2
    check_failures+=("${name}:${status}")
  fi
  return 0
}

require_last_independent_check_passed() {
  if (( $# != 1 )); then
    printf 'error: last-check assertion requires exactly one check name\n' >&2
    return 2
  fi
  local expected_name="$1"
  if [[ "${last_check_name}" != "${expected_name}" ]]; then
    printf 'error: last independent check was %s, expected %s\n' \
      "${last_check_name:-<none>}" "${expected_name}" >&2
    return 2
  fi
  [[ "${last_check_status}" == 0 ]]
}

require_independent_checks_passed() {
  if [[ "${#check_failures[@]}" -eq 0 ]]; then
    printf 'STAGE27_INDEPENDENT_CHECKS=PASS\n'
    return 0
  fi
  printf 'STAGE27_INDEPENDENT_CHECKS=FAIL count=%s\n' "${#check_failures[@]}" >&2
  printf 'STAGE27_FAILED_CHECK=%s\n' "${check_failures[@]}" >&2
  return 1
}

verify_deployed_assets() {
  [[ -f /etc/craxii/config.toml && ! -L /etc/craxii/config.toml ]] ||
    fail "deployed config path is unsafe"
  [[ -f "${asset_directory}/render-config.py" &&
     ! -L "${asset_directory}/render-config.py" ]] ||
    fail "production config renderer is absent or unsafe"
  [[ "$(stat -c '%U:%G:%a:%h' /etc/craxii/config.toml)" == root:craxii-server:640:1 ]] ||
    fail "deployed config metadata mismatch"
  [[ -f /etc/systemd/system/craxii-server.service &&
     ! -L /etc/systemd/system/craxii-server.service ]] || fail "deployed systemd unit path is unsafe"
  [[ "$(stat -c '%U:%G:%a:%h' /etc/systemd/system/craxii-server.service)" == root:root:644:1 ]] ||
    fail "deployed systemd unit metadata mismatch"
  [[ -L /opt/craxii/current ]] || fail "active release pointer is not a symbolic link"
  /usr/bin/python3 "${asset_directory}/render-config.py" \
    --template "${asset_directory}/config.toml.template" \
    --preserve-telegram-from /etc/craxii/config.toml \
    --check /etc/craxii/config.toml ||
    fail "deployed config differs from the rendered audited template"
  cmp -s "${asset_directory}/craxii-server.service" /etc/systemd/system/craxii-server.service ||
    fail "deployed systemd unit differs from the audited unit"
  [[ "$(systemctl show "${service}" --property FragmentPath --value)" == \
     /etc/systemd/system/craxii-server.service ]] ||
    fail "systemd loaded the service from an unexpected fragment"
  [[ -z "$(systemctl show "${service}" --property DropInPaths --value)" ]] ||
    fail "unexpected systemd drop-in changes production composition"
  [[ "$(systemctl show "${service}" --property NeedDaemonReload --value)" == no ]] ||
    fail "systemd manager state does not match the deployed unit"
  /bin/bash "${asset_directory}/verify-release-manifest.sh" \
    "$(readlink -f /opt/craxii/current)" "${deployment_commit}" >/dev/null
  /usr/bin/systemd-analyze verify /etc/systemd/system/craxii-server.service
}

require_execution_cgroup_clean() {
  [[ -d "${cgroup_root}" ]] || fail "delegated execution cgroup root is absent"
  [[ -z "$(find "${cgroup_root}" -mindepth 1 -type d -print -quit)" ]] ||
    fail "execution cgroup directory residue blocks further live checks"
  [[ -z "$(find "${cgroup_root}" -mindepth 1 -name cgroup.procs -type f \
    -exec awk 'NF { print; exit }' {} +)" ]] ||
    fail "execution cgroup process residue blocks further live checks"
}

print_summary() {
  local evidence="$1"
  local status="$2"
  /usr/bin/python3 - "${evidence}" "${status}" <<'PY'
import json
import pathlib
import sys

evidence = json.loads(pathlib.Path(sys.argv[1]).read_text(encoding="utf-8"))
database = evidence["database"]
artifact = database["artifact_sentinel"]
lines = [
    f"CRAXII_ID={database['craxii_id']}",
    f"RUNTIME_INSTANCE_ID={database['current_runtime']['runtime_instance_id']}",
    f"LINUX_BOOT_ID={evidence['host']['linux_boot_id']}",
    f"RELEASE_PATH={evidence['deployment']['release_path']}",
    f"SCHEMA_VERSION={database['applied_schema_version']}",
    f"DATA_FILESYSTEM_UUID={evidence['storage']['data_filesystem_uuid']}",
    f"WORKSPACE_SENTINEL_SHA256={evidence['workspace_sentinel']['sha256']}",
    f"ARTIFACT_SENTINEL_ID={artifact['artifact_id'] if artifact else 'none'}",
    f"ARTIFACT_SENTINEL_SHA256={artifact['sha256'] if artifact else 'none'}",
    f"ACTIVE_WORK={database['ambiguity']['active_work']}",
    f"INTERRUPTED_WORK={database['ambiguity']['interrupted_work']}",
    f"MODEL_OUTCOME_UNKNOWN={database['ambiguity']['model_outcome_unknown']}",
    f"TOOL_OUTCOME_UNKNOWN={database['ambiguity']['tool_outcome_unknown']}",
    f"EVIDENCE_FILE={sys.argv[1]}",
    sys.argv[2],
]
print("\n".join(lines))
PY
}

emit_pre_reboot_success() {
  if (( $# != 1 )); then
    printf 'error: pre-reboot success emission requires one evidence path\n' >&2
    return 2
  fi
  print_summary "$1" STAGE27_PRE_REBOOT_GATE=PASS
  printf 'HUMAN_EC2_REBOOT_REQUIRED\n'
}

prepare_sentinels() {
  if [[ -e "${workspace_sentinel}" || -L "${workspace_sentinel}" ]]; then
    [[ -f "${workspace_sentinel}" && ! -L "${workspace_sentinel}" ]] ||
      fail "workspace sentinel path is unsafe"
  else
    runuser -u craxii -- /usr/bin/env -i PATH=/usr/bin:/bin \
      /bin/bash --noprofile --norc -c \
      'umask 077; printf "%s\n" craxii-stage27-workspace-persistence-sentinel-v1 >"$1"' \
      stage27-sentinel "${workspace_sentinel}"
  fi

  if [[ -e "${evidence_sentinel}" || -L "${evidence_sentinel}" ]]; then
    [[ -f "${evidence_sentinel}" && ! -L "${evidence_sentinel}" ]] ||
      fail "evidence sentinel path is unsafe"
  else
    umask 077
    printf '%s\n' craxii-stage27-evidence-persistence-sentinel-v1 >"${evidence_sentinel}"
    chown root:root "${evidence_sentinel}"
    chmod 0400 "${evidence_sentinel}"
  fi
  [[ "$(stat -c '%U:%G:%a' "${evidence_sentinel}")" == root:root:400 ]] ||
    fail "evidence sentinel metadata mismatch"
  "${python}" "${evidence_helper}" validate-sentinels
}

verify_source_matches_deployment() {
  local source_commit
  [[ -d "${checkout}/.git" ]] || fail "controlled source checkout is absent"
  [[ -x "${cargo}" ]] || fail "controlled Rust toolchain is absent"
  [[ -z "$(build_git status --porcelain=v1 --untracked-files=normal)" ]] ||
    fail "controlled source checkout is dirty"
  source_commit="$(build_git rev-parse HEAD)"
  [[ "${source_commit}" =~ ^[0-9a-f]{40}$ ]] || fail "controlled source revision is invalid"
  [[ "${source_commit}" == "${deployment_commit}" ]] ||
    fail "verification checkout must exactly match the deployed release commit"
  [[ -f "${checkout}/backend/tests/stage27.rs" ]] || fail "Stage 27 live-host test is absent"
}

compile_host_tests() {
  local cargo_json="${runtime_directory}/cargo-test-artifacts.jsonl"
  local binary_list="${runtime_directory}/test-binaries.txt"
  run_build test --locked --features test-failpoints -p craxii-server \
    --lib --test stage27 --no-run --message-format=json >"${cargo_json}"
  /usr/bin/python3 - "${cargo_json}" >"${binary_list}" <<'PY'
import json
import pathlib
import sys

executables = {"live": [], "unit": []}
for line in pathlib.Path(sys.argv[1]).read_text(encoding="utf-8").splitlines():
    try:
        record = json.loads(line)
    except json.JSONDecodeError:
        continue
    target = record.get("target", {})
    executable = record.get("executable")
    if record.get("reason") != "compiler-artifact" or not executable:
        continue
    if target.get("name") == "stage27" and target.get("kind") == ["test"]:
        executables["live"].append(executable)
    if (
        target.get("name") == "craxii_server"
        and target.get("kind") == ["lib"]
        and record.get("profile", {}).get("test") is True
    ):
        executables["unit"].append(executable)
if any(len(paths) != 1 for paths in executables.values()):
    raise SystemExit("expected exactly one Stage 27 integration and library test executable")
print(executables["live"][0])
print(executables["unit"][0])
PY
  test_binaries=()
  while IFS= read -r binary; do
    [[ -n "${binary}" ]] || fail "compiled Stage 27 test executable path is empty"
    test_binaries+=("${binary}")
  done <"${binary_list}"
  [[ "${#test_binaries[@]}" -eq 2 ]] || fail "compiled Stage 27 test executables are ambiguous"
  test_binary="${test_binaries[0]}"
  unit_test_binary="${test_binaries[1]}"
  [[ -f "${test_binary}" && -x "${test_binary}" ]] || fail "compiled Stage 27 test binary is absent"
  [[ -f "${unit_test_binary}" && -x "${unit_test_binary}" ]] ||
    fail "compiled library test binary is absent"
  installed_test="${runtime_directory}/stage27-live-host-test"
  installed_unit_test="${runtime_directory}/stage27-library-test"
  install -o root -g craxii-server -m 0550 "${test_binary}" "${installed_test}"
  install -o root -g craxii-server -m 0550 "${unit_test_binary}" "${installed_unit_test}"
  readonly installed_test
  readonly installed_unit_test
}

run_unit_test() {
  local test_name="$1"
  run_build test --locked --features test-failpoints -p craxii-server --lib \
    "${test_name}" -- --exact
}

run_delegated_test() {
  (( $# >= 3 )) || fail "delegated test runner requires executable, ignored mode, and test name"
  local test_executable="$1"
  local include_ignored="$2"
  local test_name="$3"
  shift 3
  local server_uid server_gid
  local -a ignored_option=()
  [[ -f "${test_executable}" && -x "${test_executable}" ]] ||
    fail "delegated test executable is absent"
  case "${include_ignored}" in
    yes) ignored_option=(--ignored) ;;
    no) ;;
    *) fail "delegated test ignored mode is invalid" ;;
  esac
  server_uid="$(id -u craxii-server)"
  server_gid="$(id -g craxii-server)"
  (
    cd /var/lib/craxii
    umask 077
    # Enter the production delegation before dropping privileges. The non-root worker and every
    # LocalWorkstation child can then move only within the subtree systemd delegated to Craxii.
    printf '0\n' >"${cgroup_root}/cgroup.procs"
    exec /usr/bin/prlimit --nofile=65536:65536 --nproc=16384:16384 --core=0:0 \
      /usr/bin/setpriv \
      --reuid="${server_uid}" --regid="${server_gid}" --clear-groups \
      --bounding-set=-all,+kill,+setgid,+setuid,+setpcap \
      --inh-caps=-all,+kill --ambient-caps=-all,+kill \
      /usr/bin/env -i \
      HOME=/var/lib/craxii \
      USER=craxii-server \
      LOGNAME=craxii-server \
      PATH=/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin \
      CRAXII_STAGE27_LIVE_HOST=1 \
      CRAXII_STAGE27_DEPLOYMENT_COMMIT="${deployment_commit}" \
      CRAXII_STAGE27_USER_SWITCH_LAUNCHER="${launcher}" \
      CRAXII_STAGE27_CGROUP_ROOT="${cgroup_root}" \
      "$@" \
      "${test_executable}" "${ignored_option[@]}" --exact "${test_name}" --nocapture
  )
}

run_live_test() {
  local test_name="$1"
  shift
  run_delegated_test "${installed_test}" yes "${test_name}" "$@"
}

run_live_unit_test() {
  local test_name="$1"
  run_delegated_test "${installed_unit_test}" no "${test_name}"
}

move_live_test_to_verifier_cgroup() {
  local test_pid="$1"
  local verifier_cgroup verifier_cgroup_procs
  [[ "${test_pid}" =~ ^[1-9][0-9]*$ ]] || fail "live test PID is invalid"
  verifier_cgroup="$(awk -F: '$1 == "0" { print $3 }' /proc/self/cgroup)"
  [[ "${verifier_cgroup}" == /* && "${verifier_cgroup}" != *..* ]] ||
    fail "verifier cgroup path is invalid"
  [[ "${verifier_cgroup}" != /system.slice/craxii-server.service &&
     "${verifier_cgroup}" != /system.slice/craxii-server.service/* ]] ||
    fail "verifier controller unexpectedly entered the service cgroup"
  if [[ "${verifier_cgroup}" == / ]]; then
    verifier_cgroup_procs=/sys/fs/cgroup/cgroup.procs
  else
    verifier_cgroup_procs="/sys/fs/cgroup${verifier_cgroup}/cgroup.procs"
  fi
  [[ -f "${verifier_cgroup_procs}" ]] || fail "verifier cgroup.procs is absent"
  # Root owns this one controller transition. The worker used production-equivalent authority for
  # construction and spawn; moving it out now lets it observe systemd killing only the execution.
  printf '%s\n' "${test_pid}" >"${verifier_cgroup_procs}"
  grep -Fxq "0::${verifier_cgroup}" "/proc/${test_pid}/cgroup" ||
    fail "live test worker did not leave the service cgroup"
}

run_pre_reboot() {
  run_independent_check storage-layout /bin/bash \
    "${asset_directory}/bootstrap-data-volume.sh" --verify-only
  run_independent_check deployed-assets verify_deployed_assets
  run_independent_check service-active systemctl is-active --quiet "${service}"
  run_independent_check service-enabled systemctl is-enabled --quiet "${service}"
  run_independent_check service-ready wait_ready
  run_independent_check source-parity verify_source_matches_deployment
  require_independent_checks_passed || fail "read-only preflight reported independent failures"

  prepare_evidence_directory
  [[ ! -e "${pre_reboot_evidence}" && ! -L "${pre_reboot_evidence}" ]] ||
    fail "pre-reboot evidence already exists; preserve and review it instead of overwriting"
  [[ ! -e "${restart_comparison}" && ! -L "${restart_comparison}" ]] ||
    fail "restart comparison already exists; preserve and review it instead of overwriting"
  prepare_sentinels

  before_restart="${runtime_directory}/before-restart.json"
  snapshot before-service-restart "${before_restart}"

  compile_host_tests
  run_independent_check recovery-idempotency run_unit_test \
    adapters::sqlite::stage10_tests::process_loss_between_recovery_units_is_idempotent_on_the_next_startup
  run_independent_check crash-after-spawn run_live_unit_test \
    adapters::sqlite::stage8_tests::crash_after_tool_process_spawn_records_one_side_effect
  run_independent_check post-crash-cgroup-clean require_execution_cgroup_clean
  require_last_independent_check_passed post-crash-cgroup-clean ||
    fail "unsafe cgroup residue prevents further tests"
  run_independent_check outcome-unknown-no-redispatch run_unit_test \
    application::tool_execution_service::tests::cleanup_ambiguity_and_handler_panic_commit_outcome_unknown_without_redispatch
  run_independent_check linux-terminal-outcome-matrix run_live_test \
    live_linux_terminal_outcome_matrix_is_canonical
  run_independent_check post-outcome-matrix-cgroup-clean require_execution_cgroup_clean
  require_last_independent_check_passed post-outcome-matrix-cgroup-clean ||
    fail "unsafe cgroup residue prevents further tests"
  run_independent_check linux-cancellation run_live_test \
    live_linux_cancellation_cleans_process_tree_and_preserves_follower
  run_independent_check post-cancellation-cgroup-clean require_execution_cgroup_clean
  require_last_independent_check_passed post-cancellation-cgroup-clean ||
    fail "unsafe cgroup residue prevents service restart validation"
  require_independent_checks_passed || fail "isolated checks reported consolidated failures"

  control_uid="$(id -u ssm-user)"
  control_gid="$(id -g ssm-user)"
  /usr/bin/setpriv --reuid="${control_uid}" --regid="${control_gid}" --clear-groups \
    /bin/sleep infinity &
  control_pid=$!
  control_start="$(awk '{print $22}' "/proc/${control_pid}/stat")"
  if grep -Eq '^[^:]*:[^:]*:/system\.slice/craxii-server\.service(/|$)' \
    "/proc/${control_pid}/cgroup"; then
    fail "non-service control process unexpectedly entered service cgroup"
  fi

  restart_ready="${runtime_directory}/restart-ready"
  restart_done="${runtime_directory}/restart-done"
  run_live_test live_systemd_restart_kills_delegated_execution_but_not_verifier \
    CRAXII_STAGE27_RESTART_READY="${restart_ready}" \
    CRAXII_STAGE27_RESTART_DONE="${restart_done}" &
  live_test_pid=$!
  wait_for_path "${restart_ready}"
  kill -0 "${live_test_pid}" 2>/dev/null || fail "service-restart verifier exited before restart"
  move_live_test_to_verifier_cgroup "${live_test_pid}"

  systemctl restart "${service}"
  systemctl is-active --quiet "${service}" || fail "service did not restart"
  wait_ready
  install -o craxii-server -g craxii-server -m 0600 /dev/null "${restart_done}"
  wait "${live_test_pid}" || fail "delegated execution did not cleanly observe service restart"
  live_test_pid=""

  kill -0 "${control_pid}" 2>/dev/null || fail "service restart killed non-service control process"
  [[ "$(awk '{print $22}' "/proc/${control_pid}/stat")" == "${control_start}" ]] ||
    fail "non-service control PID identity changed"
  kill "${control_pid}"
  wait "${control_pid}" 2>/dev/null || true
  control_pid=""

  snapshot pre-reboot "${pre_reboot_evidence}" \
    --validation A.persistence \
    --validation B.systemd-cgroup-cleanup \
    --validation C.startup-recovery \
    --validation D.outcome-unknown-no-redispatch \
    --validation E.real-linux-cancellation \
    --validation F.graceful-service-restart \
    --validation G.pre-reboot-evidence \
    --validation H.terminal-outcome-matrix \
    --validation I.production-composition
  "${python}" "${evidence_helper}" compare \
    --mode restart \
    --before "${before_restart}" \
    --after "${pre_reboot_evidence}" \
    --output "${restart_comparison}"
  emit_pre_reboot_success "${pre_reboot_evidence}"
}

run_post_reboot() {
  run_independent_check post-reboot-storage /bin/bash \
    "${asset_directory}/bootstrap-data-volume.sh" --verify-only
  run_independent_check post-reboot-assets verify_deployed_assets
  run_independent_check post-reboot-enabled systemctl is-enabled --quiet "${service}"
  run_independent_check post-reboot-ready wait_ready
  require_independent_checks_passed || fail "post-reboot read-only checks reported independent failures"
  prepare_evidence_directory
  [[ -f "${pre_reboot_evidence}" && ! -L "${pre_reboot_evidence}" ]] ||
    fail "persistent pre-reboot evidence is absent"
  [[ -f "${restart_comparison}" && ! -L "${restart_comparison}" ]] ||
    fail "service-restart comparison evidence is absent"
  [[ ! -e "${post_reboot_evidence}" && ! -L "${post_reboot_evidence}" ]] ||
    fail "post-reboot evidence already exists; preserve and review it instead of overwriting"
  [[ ! -e "${reboot_comparison}" && ! -L "${reboot_comparison}" ]] ||
    fail "reboot comparison already exists; preserve and review it instead of overwriting"
  snapshot post-reboot "${post_reboot_evidence}" \
    --validation post-reboot.persistence \
    --validation post-reboot.systemd-auto-start \
    --validation post-reboot.recovery-before-ready \
    --validation post-reboot.credential-boundary \
    --validation post-reboot.loopback-operation
  "${python}" "${evidence_helper}" compare \
    --mode reboot \
    --before "${pre_reboot_evidence}" \
    --after "${post_reboot_evidence}" \
    --output "${reboot_comparison}"
  print_summary "${post_reboot_evidence}" STAGE27_POST_REBOOT_GATE=PASS
  printf 'STAGE27_COMPLETE=PASS\n'
}

case "${mode}" in
  --pre-reboot) run_pre_reboot ;;
  --post-reboot) run_post_reboot ;;
esac
