#!/usr/bin/env bash
set -euo pipefail

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

[[ -x "${evidence_helper}" ]] || fail "Stage 27 evidence helper is absent or not executable"
[[ -x "${asset_directory}/bootstrap-data-volume.sh" ]] || fail "data-volume verifier is absent"
"${asset_directory}/bootstrap-data-volume.sh" --verify-only
install -d -o root -g root -m 0700 "${evidence_directory}"
install -d -o craxii-server -g craxii-server -m 0700 "${runtime_directory}"

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
  "${evidence_helper}" snapshot \
    --phase "${phase}" \
    --deployment-commit "${deployment_commit}" \
    --data-uuid "${data_uuid}" \
    --output "${output}" \
    "$@"
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
print(sys.argv[2])
print(f"CRAXII_ID={database['craxii_id']}")
print(f"RUNTIME_INSTANCE_ID={database['current_runtime']['runtime_instance_id']}")
print(f"LINUX_BOOT_ID={evidence['host']['linux_boot_id']}")
print(f"RELEASE_PATH={evidence['deployment']['release_path']}")
print(f"SCHEMA_VERSION={database['applied_schema_version']}")
print(f"DATA_FILESYSTEM_UUID={evidence['storage']['data_filesystem_uuid']}")
print(f"WORKSPACE_SENTINEL_SHA256={evidence['workspace_sentinel']['sha256']}")
artifact = database["artifact_sentinel"]
print(f"ARTIFACT_SENTINEL_ID={artifact['artifact_id'] if artifact else 'none'}")
print(f"ARTIFACT_SENTINEL_SHA256={artifact['sha256'] if artifact else 'none'}")
print(f"ACTIVE_WORK={database['ambiguity']['active_work']}")
print(f"INTERRUPTED_WORK={database['ambiguity']['interrupted_work']}")
print(f"MODEL_OUTCOME_UNKNOWN={database['ambiguity']['model_outcome_unknown']}")
print(f"TOOL_OUTCOME_UNKNOWN={database['ambiguity']['tool_outcome_unknown']}")
print(f"EVIDENCE_FILE={sys.argv[1]}")
PY
}

prepare_sentinels() {
  if [[ -e "${workspace_sentinel}" || -L "${workspace_sentinel}" ]]; then
    [[ -f "${workspace_sentinel}" && ! -L "${workspace_sentinel}" ]] ||
      fail "workspace sentinel path is unsafe"
    [[ "$(<"${workspace_sentinel}")" == craxii-stage27-workspace-persistence-sentinel-v1 ]] ||
      fail "workspace sentinel content mismatch"
  else
    runuser -u craxii -- /usr/bin/env -i PATH=/usr/bin:/bin \
      /bin/bash --noprofile --norc -c \
      'umask 077; printf "%s\n" craxii-stage27-workspace-persistence-sentinel-v1 >"$1"' \
      stage27-sentinel "${workspace_sentinel}"
  fi
  "${evidence_helper}" validate-workspace-sentinel

  if [[ -e "${evidence_sentinel}" || -L "${evidence_sentinel}" ]]; then
    [[ -f "${evidence_sentinel}" && ! -L "${evidence_sentinel}" ]] ||
      fail "evidence sentinel path is unsafe"
    [[ "$(<"${evidence_sentinel}")" == craxii-stage27-evidence-persistence-sentinel-v1 ]] ||
      fail "evidence sentinel content mismatch"
  else
    umask 077
    printf '%s\n' craxii-stage27-evidence-persistence-sentinel-v1 >"${evidence_sentinel}"
    chown root:root "${evidence_sentinel}"
    chmod 0400 "${evidence_sentinel}"
  fi
  [[ "$(stat -c '%U:%G:%a' "${evidence_sentinel}")" == root:root:400 ]] ||
    fail "evidence sentinel metadata mismatch"
}

verify_source_delta_is_test_only() {
  [[ -d "${checkout}/.git" ]] || fail "controlled source checkout is absent"
  [[ -x "${cargo}" ]] || fail "controlled Rust toolchain is absent"
  [[ -z "$(build_git status --porcelain=v1 --untracked-files=normal)" ]] ||
    fail "controlled source checkout is dirty"
  source_commit="$(build_git rev-parse HEAD)"
  [[ "${source_commit}" =~ ^[0-9a-f]{40}$ ]] || fail "controlled source revision is invalid"
  if [[ "${source_commit}" != "${deployment_commit}" ]]; then
    # The Stage 8 test module is cfg(test) at its module boundary and cannot affect release builds.
    if build_git diff --quiet "${deployment_commit}..${source_commit}" -- \
      Cargo.toml Cargo.lock backend/Cargo.toml backend/build.rs backend/migrations backend/src \
      ':(exclude)backend/src/adapters/sqlite/stage8_tests.rs'; then
      :
    else
      fail "verification checkout changes production Rust sources relative to deployed release"
    fi
  fi
  [[ -f "${checkout}/backend/tests/stage27.rs" ]] || fail "Stage 27 live-host test is absent"
}

compile_host_tests() {
  local cargo_json="${runtime_directory}/cargo-test-artifacts.jsonl"
  run_build test --locked --features test-failpoints -p craxii-server \
    --lib --test stage27 --no-run --message-format=json >"${cargo_json}"
  mapfile -t test_binaries < <(/usr/bin/python3 - "${cargo_json}" <<'PY'
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
)
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

run_live_test() {
  local test_name="$1"
  shift
  local server_uid server_gid
  server_uid="$(id -u craxii-server)"
  server_gid="$(id -g craxii-server)"
  # The verifier must survive the service restart from outside its cgroup. It uses CAP_SYS_ADMIN
  # for its pre-exec child to cross into the delegated subtree; the fixed launcher clears every
  # capability before the model-controlled Bash command starts.
  /usr/bin/setpriv \
    --reuid="${server_uid}" --regid="${server_gid}" --clear-groups \
    --inh-caps=+kill,+sys_admin --ambient-caps=+kill,+sys_admin \
    /usr/bin/env -i \
    HOME=/var/lib/craxii \
    USER=craxii-server \
    LOGNAME=craxii-server \
    PATH=/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin \
    CRAXII_STAGE27_LIVE_HOST=1 \
    CRAXII_STAGE27_USER_SWITCH_LAUNCHER="${launcher}" \
    CRAXII_STAGE27_CGROUP_ROOT="${cgroup_root}" \
    "$@" \
    "${installed_test}" --ignored --exact "${test_name}" --nocapture
}

run_live_unit_test() {
  local test_name="$1"
  local server_uid server_gid
  server_uid="$(id -u craxii-server)"
  server_gid="$(id -g craxii-server)"
  (
    printf '0\n' >"${cgroup_root}/cgroup.procs"
    exec /usr/bin/setpriv \
      --reuid="${server_uid}" --regid="${server_gid}" --clear-groups \
      --inh-caps=+kill --ambient-caps=+kill \
      /usr/bin/env -i \
      HOME=/var/lib/craxii \
      USER=craxii-server \
      LOGNAME=craxii-server \
      PATH=/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin \
      CRAXII_STAGE27_LIVE_HOST=1 \
      CRAXII_STAGE27_USER_SWITCH_LAUNCHER="${launcher}" \
      CRAXII_STAGE27_CGROUP_ROOT="${cgroup_root}" \
      "${installed_unit_test}" --exact "${test_name}" --nocapture
  )
}

run_pre_reboot() {
  [[ ! -e "${pre_reboot_evidence}" && ! -L "${pre_reboot_evidence}" ]] ||
    fail "pre-reboot evidence already exists; preserve and review it instead of overwriting"
  [[ ! -e "${restart_comparison}" && ! -L "${restart_comparison}" ]] ||
    fail "restart comparison already exists; preserve and review it instead of overwriting"
  prepare_sentinels
  systemctl is-active --quiet "${service}" || fail "service is not active"
  systemctl is-enabled --quiet "${service}" || fail "service is not enabled"
  wait_ready

  before_restart="${runtime_directory}/before-restart.json"
  snapshot before-service-restart "${before_restart}"

  verify_source_delta_is_test_only
  compile_host_tests
  run_unit_test \
    adapters::sqlite::stage10_tests::process_loss_between_recovery_units_is_idempotent_on_the_next_startup
  run_live_unit_test \
    adapters::sqlite::stage8_tests::crash_after_tool_process_spawn_records_one_side_effect
  run_unit_test \
    application::tool_execution_service::tests::cleanup_ambiguity_and_handler_panic_commit_outcome_unknown_without_redispatch
  run_live_test live_linux_cancellation_cleans_process_tree_and_preserves_follower

  control_uid="$(id -u ssm-user)"
  control_gid="$(id -g ssm-user)"
  /usr/bin/setpriv --reuid="${control_uid}" --regid="${control_gid}" --clear-groups \
    /bin/sleep 120 &
  control_pid=$!
  control_start="$(awk '{print $22}' "/proc/${control_pid}/stat")"
  grep -qv '/system.slice/craxii-server.service' "/proc/${control_pid}/cgroup" ||
    fail "non-service control process unexpectedly entered service cgroup"

  restart_ready="${runtime_directory}/restart-ready"
  restart_done="${runtime_directory}/restart-done"
  run_live_test live_systemd_restart_kills_delegated_execution_but_not_verifier \
    CRAXII_STAGE27_RESTART_READY="${restart_ready}" \
    CRAXII_STAGE27_RESTART_DONE="${restart_done}" &
  live_test_pid=$!
  wait_for_path "${restart_ready}"
  kill -0 "${live_test_pid}" 2>/dev/null || fail "service-restart verifier exited before restart"

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
    --validation G.pre-reboot-evidence
  "${evidence_helper}" compare \
    --mode restart \
    --before "${before_restart}" \
    --after "${pre_reboot_evidence}" \
    --output "${restart_comparison}"
  print_summary "${pre_reboot_evidence}" STAGE27_PRE_REBOOT_GATE=PASS
  printf 'HUMAN_EC2_REBOOT_REQUIRED\n'
}

run_post_reboot() {
  [[ -f "${pre_reboot_evidence}" && ! -L "${pre_reboot_evidence}" ]] ||
    fail "persistent pre-reboot evidence is absent"
  [[ -f "${restart_comparison}" && ! -L "${restart_comparison}" ]] ||
    fail "service-restart comparison evidence is absent"
  [[ ! -e "${post_reboot_evidence}" && ! -L "${post_reboot_evidence}" ]] ||
    fail "post-reboot evidence already exists; preserve and review it instead of overwriting"
  [[ ! -e "${reboot_comparison}" && ! -L "${reboot_comparison}" ]] ||
    fail "reboot comparison already exists; preserve and review it instead of overwriting"
  systemctl is-enabled --quiet "${service}" || fail "service is not enabled after reboot"
  wait_ready
  snapshot post-reboot "${post_reboot_evidence}" \
    --validation post-reboot.persistence \
    --validation post-reboot.systemd-auto-start \
    --validation post-reboot.recovery-before-ready \
    --validation post-reboot.credential-boundary \
    --validation post-reboot.loopback-operation
  "${evidence_helper}" compare \
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
