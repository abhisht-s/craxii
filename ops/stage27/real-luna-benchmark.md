# Stage 27 real Luna canonical benchmark

Audit result: no repository or deployment change is required.

The production route is `POST /v1/conversations/{conversation_id}/messages`,
authenticated with the provisioned device bearer and a matching
`Idempotency-Key`. Only the bearer hash is stored by Craxii, so this runbook
prompts for the retained bearer through `/dev/tty` without echo. The bearer is
not exported, written to disk, placed in process arguments, or printed.

Paste this block once into browser SSM:

```bash
sudo /bin/bash <<'STAGE27_LUNA_BENCHMARK'
set -euo pipefail
umask 077

submitted=NO
temporary_directory=""
device_bearer=""

fail() {
  if [[ "${submitted}" == NO ]]; then
    printf 'STAGE_27_REAL_LUNA_READY: NO - %s\n' "$*" >&2
  else
    printf 'STAGE27_REAL_LUNA_CANONICAL_BENCHMARK: FAILED - %s\n' "$*" >&2
  fi
  exit 1
}

cleanup() {
  device_bearer=""
  if [[ -n "${temporary_directory}" &&
        "${temporary_directory}" == /tmp/stage27-luna-benchmark.* ]]; then
    rm -f -- \
      "${temporary_directory}/request.json" \
      "${temporary_directory}/response.json"
    rmdir -- "${temporary_directory}" 2>/dev/null || true
  fi
}
trap cleanup EXIT

readonly deployment_commit=bffd69f59206f3e863b42beb0d0c386b6820cb19
readonly expected_main_pid=32706
readonly release=/opt/craxii/releases/0.0.1-bffd69f59206
readonly source_directory=/var/lib/craxii-build/source
readonly database=/var/lib/craxii/db/craxii.sqlite3
readonly config=/etc/craxii/config.toml
readonly unit=/etc/systemd/system/craxii-server.service
readonly credential=/etc/craxii/credentials/openai_provider
readonly workspace=/srv/craxii/workspaces/primary
readonly launcher=/opt/craxii/current/craxii-workstation-launcher
readonly reader=/opt/craxii/current/craxii-workstation-reader
readonly cgroup_root=\
/sys/fs/cgroup/system.slice/craxii-server.service/craxii-executions
readonly base_url=http://127.0.0.1:8080
canonical_prompt="Inspect your machine and tell me what OS, CPU architecture, "
canonical_prompt+="current directory, and Git version you have."
readonly canonical_prompt

(( EUID == 0 )) || fail "run this exact block from the browser SSM terminal"
for required_command in \
  curl find getent git grep journalctl python3 readlink runuser setpriv stat systemctl uname; do
  command -v "${required_command}" >/dev/null ||
    fail "required host command is absent: ${required_command}"
done

systemctl is-active --quiet craxii-server.service || fail "service is not active"
main_pid="$(systemctl show craxii-server.service --property MainPID --value)"
restart_count="$(systemctl show craxii-server.service --property NRestarts --value)"
[[ "${main_pid}" == "${expected_main_pid}" ]] ||
  fail "the healthy activation PID changed before submission"
[[ "${restart_count}" =~ ^[0-9]+$ ]] || fail "service restart count is unavailable"
[[ "$(stat -c %U "/proc/${main_pid}")" == craxii-server ]] ||
  fail "backend process is not craxii-server"
backend_uid="$(stat -c %u "/proc/${main_pid}")"
[[ "${backend_uid}" != 0 ]] || fail "backend process unexpectedly runs as root"
[[ "$(readlink -f "/proc/${main_pid}/exe")" == "${release}/craxii-server" ]] ||
  fail "backend executable does not match the immutable release"
[[ "$(readlink -f /opt/craxii/current)" == "${release}" ]] ||
  fail "active release symlink changed"
[[ "$(systemctl show craxii-server.service --property User --value)" ==
   craxii-server ]] || fail "systemd User differs from the production contract"
[[ "$(systemctl show craxii-server.service --property Group --value)" ==
   craxii-server ]] || fail "systemd Group differs from the production contract"
[[ "$(systemctl show craxii-server.service --property Delegate --value)" == yes ]] ||
  fail "systemd cgroup delegation is disabled"
[[ "$(systemctl show craxii-server.service --property ControlGroup --value)" ==
   /system.slice/craxii-server.service ]] || fail "unexpected service cgroup"
grep -qx '0::/system.slice/craxii-server.service' "/proc/${main_pid}/cgroup" ||
  fail "backend is outside the intended systemd cgroup"

[[ -f "${database}" && ! -L "${database}" ]] || fail "state database is absent"
[[ -f "${config}" && ! -L "${config}" ]] || fail "production config is absent"
[[ -f "${unit}" && ! -L "${unit}" ]] || fail "production unit is absent"
[[ -f "${credential}" && ! -L "${credential}" ]] ||
  fail "provider credential metadata is invalid"
[[ "$(stat -c '%U:%G:%a:%h' "${credential}")" ==
   craxii-server:craxii-server:600:1 ]] ||
  fail "provider credential metadata differs from the production contract"
(( $(stat -c %s "${credential}") > 0 )) || fail "provider credential is empty"
if runuser -u craxii -- test -r "${credential}"; then
  fail "model-controlled user can access the provider credential"
fi

[[ "$(stat -c '%U:%G:%a:%h' "${launcher}")" ==
   root:craxii-server:4750:1 ]] || fail "production launcher metadata is invalid"
[[ "$(stat -c '%U:%G:%a:%h' "${reader}")" == root:root:111:1 ]] ||
  fail "production reader metadata is invalid"
[[ "$(readlink -f "${launcher}")" ==
   "${release}/craxii-workstation-launcher" ]] ||
  fail "launcher is outside the immutable release"
[[ -d "${cgroup_root}" ]] || fail "delegated execution cgroup root is absent"
if find "${cgroup_root}" -mindepth 1 -maxdepth 1 -type d -print -quit |
   grep -q .; then
  fail "a pre-existing execution cgroup makes the benchmark state ambiguous"
fi

grep -qx 'User=craxii-server' "${unit}" || fail "unit User is incorrect"
grep -qx 'Group=craxii-server' "${unit}" || fail "unit Group is incorrect"
grep -qx 'Delegate=yes' "${unit}" || fail "unit delegation is incorrect"
grep -qx 'KillMode=control-group' "${unit}" || fail "unit KillMode is incorrect"
grep -qx 'AmbientCapabilities=CAP_KILL' "${unit}" ||
  fail "unit cancellation capability is incorrect"
grep -qx \
  'LoadCredential=openai_provider:/etc/craxii/credentials/openai_provider' \
  "${unit}" || fail "unit credential mapping is incorrect"
if grep -Eq '^Environment(File)?=' "${unit}"; then
  fail "unit contains a global environment source"
fi

build_head="$(
  runuser -u craxii-build -- /usr/bin/env -i \
    HOME=/var/lib/craxii-build \
    PATH=/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin \
    /usr/bin/git -C "${source_directory}" rev-parse HEAD
)"
[[ "${build_head}" == "${deployment_commit}" ]] ||
  fail "build checkout does not match the deployed commit"
build_dirty="$(
  runuser -u craxii-build -- /usr/bin/env -i \
    HOME=/var/lib/craxii-build \
    PATH=/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin \
    /usr/bin/git -C "${source_directory}" \
      status --porcelain=v1 --untracked-files=normal
)"
[[ -z "${build_dirty}" ]] || fail "build checkout is dirty"

/usr/bin/env -i PATH=/usr/bin:/bin CONFIG="${config}" /usr/bin/python3 <<'PY'
import os
import tomllib

def require(condition, message):
    if not condition:
        raise SystemExit(message)

with open(os.environ["CONFIG"], "rb") as stream:
    value = tomllib.load(stream)

require(value["server"]["bind_address"] == "127.0.0.1:8080", "bad bind address")
require(value["server"]["public_base_url"] == "http://127.0.0.1:8080", "bad URL")
require(value["paths"]["state_root"] == "/var/lib/craxii", "bad state root")
require(
    value["paths"]["primary_workspace_root"]
    == "/srv/craxii/workspaces/primary",
    "bad workspace root",
)
require(value["credentials"]["source"] == "systemd", "bad credential source")
telegram = value.get("telegram", {"enabled": False})
expected_credentials = (
    ["openai_provider", "telegram_bot"]
    if telegram["enabled"]
    else ["openai_provider"]
)
require(value["credentials"]["declared"] == expected_credentials, "bad credential set")
if telegram["enabled"]:
    require(telegram["credential"] == "telegram_bot", "bad Telegram credential")
require(value["models"]["default_target"] == "stage27-openai", "bad default target")
targets = value["models"]["targets"]
require(len(targets) == 1, "more than one production model target")
target = targets[0]
require(target["id"] == "stage27-openai", "bad target ID")
require(target["enabled"] is True, "target is disabled")
require(target["provider"] == "openai", "bad provider")
require(target["provider_model_id"] == "gpt-5.6-luna", "bad model")
require(target["credential"] == "openai_provider", "bad credential reference")
require(target["reasoning_continuation"] is False, "unexpected continuation mode")
shell = value["shell"]
require(shell["executable"] == "/bin/bash", "bad shell")
require(shell["environment_policy"] == "clean", "bad environment policy")
require(shell["inherited_variables"] == [], "inherited variables are configured")
require(shell["administrative_enabled"] is False, "administrative tools are enabled")
require(
    shell["user_switch_launcher"]
    == "/opt/craxii/current/craxii-workstation-launcher",
    "bad launcher",
)
require(
    shell["delegated_cgroup_root"]
    == "/sys/fs/cgroup/system.slice/craxii-server.service/craxii-executions",
    "bad cgroup root",
)

def contains_fallback_key(item):
    if isinstance(item, dict):
        return any(
            "fallback" in str(key).lower() or contains_fallback_key(child)
            for key, child in item.items()
        )
    if isinstance(item, list):
        return any(contains_fallback_key(child) for child in item)
    return False

require(not contains_fallback_key(value), "fallback configuration is present")
PY

health_live_before="$(
  /usr/bin/env -i PATH=/usr/bin:/bin /usr/bin/curl \
    --silent --show-error --output /dev/null --write-out '%{http_code}' \
    --connect-timeout 2 --max-time 5 "${base_url}/health/live"
)"
health_ready_before="$(
  /usr/bin/env -i PATH=/usr/bin:/bin /usr/bin/curl \
    --silent --show-error --output /dev/null --write-out '%{http_code}' \
    --connect-timeout 2 --max-time 5 "${base_url}/health/ready"
)"
[[ "${health_live_before}" == 200 ]] || fail "initial liveness check failed"
[[ "${health_ready_before}" == 200 ]] || fail "initial readiness check failed"

preflight_values="$(
  /usr/bin/env -i PATH=/usr/bin:/bin \
    DB_PATH="${database}" MAIN_PID="${main_pid}" \
    DEPLOYMENT_COMMIT="${deployment_commit}" \
    /usr/bin/python3 <<'PY'
import os
import sqlite3

def require(condition, message):
    if not condition:
        raise SystemExit(message)

connection = sqlite3.connect(
    f"file:{os.environ['DB_PATH']}?mode=ro",
    uri=True,
    timeout=5,
)
connection.row_factory = sqlite3.Row
principal_rows = connection.execute(
    "SELECT craxii_id, primary_conversation_id, default_workspace_id "
    "FROM craxii_principals WHERE lifecycle_state = 'active'"
).fetchall()
require(len(principal_rows) == 1, "active Craxii principal cardinality differs")
principal = principal_rows[0]
conversation = connection.execute(
    "SELECT conversation_id, next_work_ordinal FROM conversations "
    "WHERE conversation_id = ? AND kind = 'primary' AND lifecycle_state = 'active'",
    (principal["primary_conversation_id"],),
).fetchone()
require(conversation is not None, "primary conversation is absent")
require(conversation["next_work_ordinal"] == 1, "conversation is not pristine")
workspace = connection.execute(
    "SELECT workspace_id, workstation_id, logical_root, local_resolved_root "
    "FROM workspaces WHERE workspace_id = ? AND logical_name = 'primary' "
    "AND lifecycle_state = 'active'",
    (principal["default_workspace_id"],),
).fetchone()
require(workspace is not None, "primary workspace is absent")
require(
    workspace["logical_root"] == "/srv/craxii/workspaces/primary"
    and workspace["local_resolved_root"] == "/srv/craxii/workspaces/primary",
    "workspace identity differs",
)
runtime_rows = connection.execute(
    "SELECT runtime_instance_id, workstation_id, process_id, git_revision "
    "FROM runtime_instances WHERE state = 'running'"
).fetchall()
require(len(runtime_rows) == 1, "running runtime cardinality differs")
runtime = runtime_rows[0]
require(runtime["process_id"] == int(os.environ["MAIN_PID"]), "runtime PID differs")
require(
    runtime["git_revision"] == os.environ["DEPLOYMENT_COMMIT"],
    "runtime commit differs",
)
require(runtime["workstation_id"] == workspace["workstation_id"], "runtime workstation differs")
active_devices = connection.execute(
    "SELECT COUNT(*) FROM client_devices WHERE revoked_at IS NULL"
).fetchone()[0]
require(active_devices >= 1, "no active provisioned device exists")
for table in (
    "messages",
    "work_items",
    "work_item_inputs",
    "client_commands",
    "model_invocations",
    "tool_executions",
):
    count = connection.execute(f"SELECT COUNT(*) FROM {table}").fetchone()[0]
    require(count == 0, f"{table} is not pristine")
print(
    conversation["conversation_id"],
    workspace["workspace_id"],
    runtime["runtime_instance_id"],
    workspace["workstation_id"],
    principal["craxii_id"],
    sep="\t",
)
PY
)" || fail "durable preflight failed"
IFS=$'\t' read -r \
  conversation_id workspace_id runtime_instance_id workstation_id craxii_id \
  <<<"${preflight_values}"
readonly uuid_pattern='^[0-9a-f]{8}-[0-9a-f]{4}-7[0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}$'
for durable_id in \
  "${conversation_id}" "${workspace_id}" "${runtime_instance_id}" \
  "${workstation_id}" "${craxii_id}"; do
  [[ "${durable_id}" =~ ${uuid_pattern} ]] || fail "preflight returned a malformed ID"
done

temporary_directory="$(mktemp -d /tmp/stage27-luna-benchmark.XXXXXX)"
[[ "${temporary_directory}" == /tmp/stage27-luna-benchmark.* ]] ||
  fail "temporary directory creation failed"
client_message_id="$(
  /usr/bin/env -i PATH=/usr/bin:/bin /usr/bin/python3 <<'PY'
import os
import time
import uuid

milliseconds = int(time.time() * 1000)
value = bytearray(milliseconds.to_bytes(6, "big") + os.urandom(10))
value[6] = (value[6] & 0x0F) | 0x70
value[8] = (value[8] & 0x3F) | 0x80
print(uuid.UUID(bytes=bytes(value)))
PY
)"
[[ "${client_message_id}" =~ ${uuid_pattern} ]] || fail "UUIDv7 generation failed"

/usr/bin/env -i PATH=/usr/bin:/bin \
  REQUEST_PATH="${temporary_directory}/request.json" \
  CLIENT_MESSAGE_ID="${client_message_id}" PROMPT="${canonical_prompt}" \
  /usr/bin/python3 <<'PY'
import json
import os

request = {
    "protocol_version": 1,
    "client_message_id": os.environ["CLIENT_MESSAGE_ID"],
    "content": [{"type": "text", "text": os.environ["PROMPT"]}],
}
with open(os.environ["REQUEST_PATH"], "x", encoding="utf-8") as stream:
    json.dump(request, stream, separators=(",", ":"))
    stream.write("\n")
PY

IFS= read -r -s -p \
  'Paste the existing 64-character local device bearer (input hidden): ' \
  device_bearer </dev/tty || fail "device bearer input was cancelled"
printf '\n' >/dev/tty
[[ "${device_bearer}" =~ ^[0-9a-f]{64}$ ]] ||
  fail "device bearer must be exactly 64 lowercase hexadecimal characters"

bootstrap_http="$(
  {
    printf '%s\n' silent show-error
    printf 'header = "Authorization: Bearer %s"\n' "${device_bearer}"
  } | /usr/bin/env -i PATH=/usr/bin:/bin /usr/bin/curl --config - \
    --output /dev/null --write-out '%{http_code}' \
    --connect-timeout 2 --max-time 10 "${base_url}/v1/bootstrap"
)" || fail "authenticated bootstrap request failed"
[[ "${bootstrap_http}" == 200 ]] || fail "device bearer did not authenticate"

benchmark_start_epoch="$(date +%s)"
readonly benchmark_start_epoch
submitted=YES
set +e
post_http="$(
  {
    printf '%s\n' silent show-error
    printf 'header = "Authorization: Bearer %s"\n' "${device_bearer}"
  } | /usr/bin/env -i PATH=/usr/bin:/bin /usr/bin/curl --config - \
    --request POST \
    --header 'Content-Type: application/json' \
    --header "Idempotency-Key: ${client_message_id}" \
    --data-binary "@${temporary_directory}/request.json" \
    --output "${temporary_directory}/response.json" \
    --write-out '%{http_code}' --connect-timeout 2 --max-time 30 \
    "${base_url}/v1/conversations/${conversation_id}/messages"
)"
post_status=$?
set -e

set +e
command_values="$(
  /usr/bin/env -i PATH=/usr/bin:/bin \
    DB_PATH="${database}" CLIENT_MESSAGE_ID="${client_message_id}" \
    /usr/bin/python3 <<'PY'
import json
import os
import sqlite3
import time

connection = sqlite3.connect(
    f"file:{os.environ['DB_PATH']}?mode=ro",
    uri=True,
    timeout=5,
)
connection.row_factory = sqlite3.Row
deadline = time.monotonic() + 15
row = None
while time.monotonic() < deadline:
    row = connection.execute(
        "SELECT response_http_status, committed_cursor, response_json "
        "FROM client_commands WHERE idempotency_key = ? AND command_type = 'message'",
        (os.environ["CLIENT_MESSAGE_ID"],),
    ).fetchone()
    if row is not None:
        break
    time.sleep(0.2)
if row is None:
    raise SystemExit(3)
try:
    response = json.loads(row["response_json"])
    values = (
        row["response_http_status"],
        response["message_id"],
        response["work_id"],
        row["committed_cursor"],
        str(response["duplicate"]).lower(),
    )
except (KeyError, TypeError, ValueError, json.JSONDecodeError):
    raise SystemExit(4)
print(*values, sep="\t")
PY
)"
command_lookup_status=$?
set -e
if (( command_lookup_status != 0 )); then
  if (( command_lookup_status == 3 )); then
    printf 'STAGE27_SUBMISSION_STATUS=AMBIGUOUS\n' >&2
    printf 'STAGE27_CLIENT_MESSAGE_ID=%s\n' "${client_message_id}" >&2
    printf 'STAGE27_ACTION=DO_NOT_RESUBMIT_DIAGNOSE_DURABLE_STATE\n' >&2
    exit 1
  fi
  fail "durable client-command evidence is malformed"
fi
IFS=$'\t' read -r \
  accepted_http message_id work_id accepted_cursor duplicate \
  <<<"${command_values}"
[[ "${accepted_http}" == 202 ]] || fail "durable command status is not 202"
[[ "${duplicate}" == false ]] || fail "the benchmark command was a replay"
[[ "${message_id}" =~ ${uuid_pattern} && "${work_id}" =~ ${uuid_pattern} ]] ||
  fail "accepted durable IDs are malformed"
[[ "${accepted_cursor}" =~ ^[1-9][0-9]*$ ]] ||
  fail "accepted durable cursor is malformed"

if (( post_status == 0 )) && [[ "${post_http}" == 202 ]]; then
  /usr/bin/env -i PATH=/usr/bin:/bin \
    RESPONSE_PATH="${temporary_directory}/response.json" \
    MESSAGE_ID="${message_id}" WORK_ID="${work_id}" \
    /usr/bin/python3 <<'PY'
import json
import os

with open(os.environ["RESPONSE_PATH"], encoding="utf-8") as stream:
    response = json.load(stream)
if response.get("message_id") != os.environ["MESSAGE_ID"]:
    raise SystemExit("HTTP message ID differs from durable evidence")
if response.get("work_id") != os.environ["WORK_ID"]:
    raise SystemExit("HTTP work ID differs from durable evidence")
if response.get("duplicate") is not False:
    raise SystemExit("HTTP response unexpectedly reports a replay")
PY
fi

work_terminal="$(
  /usr/bin/env -i PATH=/usr/bin:/bin \
    DB_PATH="${database}" WORK_ID="${work_id}" MAIN_PID="${main_pid}" \
    EXPECTED_EXE="${release}/craxii-server" \
    /usr/bin/python3 <<'PY'
import os
import sqlite3
import time

connection = sqlite3.connect(
    f"file:{os.environ['DB_PATH']}?mode=ro",
    uri=True,
    timeout=5,
)
deadline = time.monotonic() + 1860
pid = os.environ["MAIN_PID"]
while time.monotonic() < deadline:
    try:
        executable = os.path.realpath(f"/proc/{pid}/exe")
    except OSError:
        executable = ""
    if executable != os.environ["EXPECTED_EXE"]:
        raise SystemExit("service activation changed while work was pending")
    row = connection.execute(
        "SELECT state, terminal_reason_code FROM work_items WHERE work_id = ?",
        (os.environ["WORK_ID"],),
    ).fetchone()
    if row is None:
        raise SystemExit("accepted work disappeared")
    if row[0] == "completed":
        print(row[0], row[1], sep="\t")
        break
    if row[0] in ("failed", "cancelled", "interrupted"):
        raise SystemExit(f"work became terminal: {row[0]}/{row[1]}")
    time.sleep(1)
else:
    raise SystemExit("work did not become terminal before the bounded deadline")
PY
)" || fail "benchmark work did not complete safely"
IFS=$'\t' read -r work_state terminal_reason <<<"${work_terminal}"
[[ "${work_state}" == completed && "${terminal_reason}" == answered ]] ||
  fail "work terminal state is not completed/answered"

independent_os="$(
  /usr/bin/env -i PATH=/usr/bin:/bin /usr/bin/python3 <<'PY'
import shlex

values = {}
with open("/etc/os-release", encoding="utf-8") as stream:
    for line in stream:
        line = line.rstrip("\n")
        if "=" not in line or line.startswith("#"):
            continue
        key, raw = line.split("=", 1)
        parsed = shlex.split(raw)
        values[key] = parsed[0] if parsed else ""
if values.get("ID") != "ubuntu" or values.get("VERSION_ID") != "24.04":
    raise SystemExit("independent OS identity differs from Ubuntu 24.04")
print(f"{values.get('PRETTY_NAME', 'Ubuntu 24.04')}/Linux")
PY
)" || fail "independent OS measurement failed"
independent_arch="$(/usr/bin/env -i PATH=/usr/bin:/bin /usr/bin/uname -m)"
independent_cwd="$(
  cd "${workspace}"
  /bin/pwd -P
)"
independent_git="$(
  cd "${workspace}"
  runuser -u craxii -- /usr/bin/env -i \
    HOME=/home/craxii \
    PATH=/home/craxii/.local/bin:/home/craxii/.cargo/bin:/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin \
    /usr/bin/git --version
)"
[[ "${independent_arch}" == x86_64 ]] || fail "independent architecture differs"
[[ "${independent_cwd}" == "${workspace}" ]] || fail "independent cwd differs"
[[ "${independent_git}" == 'git version '* ]] ||
  fail "independent Git measurement failed"

workstation_uid="$(id -u craxii)"
server_uid="$(id -u craxii-server)"
server_gid="$(id -g craxii-server)"
launcher_test=""
read -r -d '' launcher_test <<'STAGE27_LAUNCHER_CHILD' || true
provider=NO
directory=NO
canary=NO
[[ -v OPENAI_API_KEY ]] && provider=YES
[[ -v CREDENTIALS_DIRECTORY ]] && directory=YES
[[ -v CRAXII_STAGE27_PROVIDER_CANARY ]] && canary=YES
test ! -e /proc/self/fd/9
grep -Eq '^Groups:[[:space:]]*$' /proc/self/status
grep -Eq '^CapEff:[[:space:]]*0+$' /proc/self/status
grep -Eq '^CapAmb:[[:space:]]*0+$' /proc/self/status
grep -Eq '^NoNewPrivs:[[:space:]]*1$' /proc/self/status
printf '%s\t%s\t%s\t%s\t%s\t%s\n' \
  "$(id -u)" "$(id -un)" "$(pwd -P)" \
  "$provider" "$directory" "$canary"
STAGE27_LAUNCHER_CHILD
launcher_probe="$(
  cd "${workspace}"
  /usr/bin/setpriv \
    --reuid="${server_uid}" --regid="${server_gid}" --clear-groups \
    --inh-caps=+kill --ambient-caps=+kill \
    /usr/bin/env -i \
    OPENAI_API_KEY=synthetic-stage27-provider-canary \
    CREDENTIALS_DIRECTORY=/synthetic/stage27-credentials \
    CRAXII_STAGE27_PROVIDER_CANARY=synthetic-stage27-provider-canary \
    /bin/bash --noprofile --norc -c \
    'exec 9</etc/hostname; exec "$1" shell "$2" "$3" "$4"' \
    stage27-launcher-probe "${launcher}" "${work_id}" "${workspace_id}" \
    "${launcher_test}"
)" || fail "production launcher boundary probe failed"
IFS=$'\t' read -r \
  child_uid child_user child_cwd child_provider child_directory child_canary \
  <<<"${launcher_probe}"
[[ "${child_uid}" == "${workstation_uid}" && "${child_uid}" != 0 ]] ||
  fail "launcher child UID is incorrect"
[[ "${child_uid}" != "${server_uid}" && "${child_user}" == craxii ]] ||
  fail "launcher did not separate backend and workstation identities"
[[ "${child_cwd}" == "${workspace}" ]] || fail "launcher child cwd is incorrect"
[[ "${child_provider}" == NO && "${child_directory}" == NO &&
   "${child_canary}" == NO ]] || fail "launcher inherited a forbidden variable"
if runuser -u craxii -- "${launcher}" shell x y true >/dev/null 2>&1; then
  fail "model-controlled user bypassed the launcher's caller guard"
fi
if "${launcher}" shell x y true >/dev/null 2>&1; then
  fail "root bypassed the launcher's real-UID guard"
fi

systemctl is-active --quiet craxii-server.service || fail "service stopped after benchmark"
post_main_pid="$(systemctl show craxii-server.service --property MainPID --value)"
post_restart_count="$(
  systemctl show craxii-server.service --property NRestarts --value
)"
[[ "${post_main_pid}" == "${main_pid}" ]] || fail "service PID changed after benchmark"
[[ "${post_restart_count}" == "${restart_count}" ]] ||
  fail "service restart count changed after benchmark"
[[ "$(readlink -f "/proc/${post_main_pid}/exe")" == "${release}/craxii-server" ]] ||
  fail "post-benchmark executable changed"
if find "${cgroup_root}" -mindepth 1 -maxdepth 1 -type d -print -quit |
   grep -q .; then
  fail "an execution cgroup remained after completed work"
fi
health_live="$(
  /usr/bin/env -i PATH=/usr/bin:/bin /usr/bin/curl \
    --silent --show-error --output /dev/null --write-out '%{http_code}' \
    --connect-timeout 2 --max-time 5 "${base_url}/health/live"
)"
health_ready="$(
  /usr/bin/env -i PATH=/usr/bin:/bin /usr/bin/curl \
    --silent --show-error --output /dev/null --write-out '%{http_code}' \
    --connect-timeout 2 --max-time 5 "${base_url}/health/ready"
)"
[[ "${health_live}" == 200 ]] || fail "post-benchmark liveness check failed"
[[ "${health_ready}" == 200 ]] || fail "post-benchmark readiness check failed"

journalctl --unit craxii-server.service \
  --since "@${benchmark_start_epoch}" --output cat --no-pager >/dev/null ||
  fail "benchmark journal interval is unavailable"
exact_bearer_leak=NO
while IFS= read -r journal_line; do
  if [[ "${journal_line}" == *"${device_bearer}"* ]]; then
    exact_bearer_leak=YES
    break
  fi
done < <(
  journalctl --unit craxii-server.service \
    --since "@${benchmark_start_epoch}" --output cat --no-pager
)
[[ "${exact_bearer_leak}" == NO ]] || fail "device bearer appeared in the journal"
device_bearer=""
unset device_bearer

/usr/bin/env -i PATH=/usr/bin:/bin \
  DB_PATH="${database}" WORK_ID="${work_id}" MESSAGE_ID="${message_id}" \
  CLIENT_MESSAGE_ID="${client_message_id}" \
  CONVERSATION_ID="${conversation_id}" WORKSPACE_ID="${workspace_id}" \
  WORKSTATION_ID="${workstation_id}" RUNTIME_ID="${runtime_instance_id}" \
  CRAXII_ID="${craxii_id}" MAIN_PID="${main_pid}" \
  DEPLOYMENT_COMMIT="${deployment_commit}" PROMPT="${canonical_prompt}" \
  HOST_OS="${independent_os}" HOST_ARCH="${independent_arch}" \
  HOST_CWD="${independent_cwd}" HOST_GIT="${independent_git}" \
  CHILD_UID="${child_uid}" CHILD_USER="${child_user}" \
  JOURNAL_SINCE_EPOCH="${benchmark_start_epoch}" \
  HEALTH_LIVE="${health_live}" HEALTH_READY="${health_ready}" \
  CGROUP_ROOT="${cgroup_root}" /usr/bin/python3 <<'PY'
import json
import os
import re
import sqlite3
import subprocess

def require(condition, message):
    if not condition:
        raise SystemExit(message)

def exactly_one(rows, message):
    require(len(rows) == 1, message)
    return rows[0]

def content_text(encoded):
    value = json.loads(encoded)
    blocks = value.get("blocks", []) if isinstance(value, dict) else value
    require(isinstance(blocks, list), "message content shape is invalid")
    return "\n".join(
        block.get("text", "")
        for block in blocks
        if isinstance(block, dict) and block.get("type") == "text"
    )

def flatten_strings(value):
    if isinstance(value, str):
        yield value
    elif isinstance(value, dict):
        for child in value.values():
            yield from flatten_strings(child)
    elif isinstance(value, list):
        for child in value:
            yield from flatten_strings(child)

def record_value(record, key):
    if key in record:
        return record[key]
    fields = record.get("fields")
    return fields.get(key) if isinstance(fields, dict) else None

def record_spans(record):
    spans = record.get("spans")
    values = list(spans) if isinstance(spans, list) else []
    current = record.get("span")
    if isinstance(current, dict) and not any(
        span.get("name") == current.get("name") for span in values
    ):
        values.append(current)
    return values

def span_value(span, key):
    if key in span:
        return span[key]
    fields = span.get("fields")
    return fields.get(key) if isinstance(fields, dict) else None

def named_span(record, name):
    matches = [span for span in record_spans(record) if span.get("name") == name]
    return exactly_one(matches, f"telemetry span is missing: {name}")

connection = sqlite3.connect(
    f"file:{os.environ['DB_PATH']}?mode=ro",
    uri=True,
    timeout=5,
)
connection.row_factory = sqlite3.Row
work_id = os.environ["WORK_ID"]
message_id = os.environ["MESSAGE_ID"]
client_message_id = os.environ["CLIENT_MESSAGE_ID"]

require(connection.execute("SELECT COUNT(*) FROM work_items").fetchone()[0] == 1,
        "durable work cardinality differs")
require(connection.execute("SELECT COUNT(*) FROM messages").fetchone()[0] == 2,
        "durable message cardinality differs")
require(connection.execute("SELECT COUNT(*) FROM client_commands").fetchone()[0] == 1,
        "durable command cardinality differs")
require(connection.execute("SELECT COUNT(*) FROM work_item_inputs").fetchone()[0] == 1,
        "durable work-input cardinality differs")

user = exactly_one(connection.execute(
    "SELECT * FROM messages WHERE role = 'user'"
).fetchall(), "durable user-message cardinality differs")
require(user["message_id"] == message_id, "accepted message ID differs")
require(user["client_message_id"] == client_message_id, "client message ID differs")
require(user["conversation_id"] == os.environ["CONVERSATION_ID"],
        "user conversation differs")
require(content_text(user["content_json"]) == os.environ["PROMPT"],
        "canonical user message differs")

work = exactly_one(connection.execute(
    "SELECT * FROM work_items WHERE work_id = ?", (work_id,)
).fetchall(), "accepted work is absent")
require(work["conversation_id"] == os.environ["CONVERSATION_ID"],
        "work conversation differs")
require(work["conversation_work_ordinal"] == 1, "work ordinal differs")
require(work["kind"] == "conversational", "work kind differs")
require(work["workspace_id"] == os.environ["WORKSPACE_ID"], "work workspace differs")
require(work["state"] == "completed", "work did not persist completed")
require(work["terminal_reason_code"] == "answered", "work reason is not answered")
require(work["started_at"] is not None and work["terminal_at"] is not None,
        "work timing evidence is incomplete")
correlation_id = work["correlation_id"]

command = exactly_one(connection.execute(
    "SELECT * FROM client_commands WHERE idempotency_key = ?",
    (client_message_id,),
).fetchall(), "durable client command is absent")
response = json.loads(command["response_json"])
require(command["command_type"] == "message", "client command type differs")
require(command["response_http_status"] == 202, "durable HTTP status differs")
require(command["device_id"] == user["client_device_id"], "authenticated device differs")
require(response.get("protocol_version") == 1, "response protocol differs")
require(response.get("message_id") == message_id, "response message ID differs")
require(response.get("work_id") == work_id, "response work ID differs")
require(response.get("conversation_work_ordinal") == 1, "response ordinal differs")
require(response.get("duplicate") is False, "response records a replay")
require(response.get("work_state") == "queued", "acceptance did not queue work")
require(response.get("committed_cursor") == command["committed_cursor"],
        "command cursor differs")

trigger = exactly_one(connection.execute(
    "SELECT i.relationship, i.ordinal_within_work, i.attached_by_actor, "
    "e.event_id, e.event_type, e.journal_offset, e.payload_json "
    "FROM work_item_inputs i JOIN journal_events e ON e.event_id = i.input_event_id "
    "WHERE i.work_id = ?", (work_id,)
).fetchall(), "durable trigger input is absent")
require(trigger["relationship"] == "trigger" and trigger["ordinal_within_work"] == 1,
        "work trigger relationship differs")
require(trigger["attached_by_actor"] == "user", "work trigger actor differs")
require(trigger["event_type"] == "message.accepted", "accepted event differs")
require(json.loads(trigger["payload_json"]).get("message_id") == message_id,
        "accepted event message differs")

assistant = exactly_one(connection.execute(
    "SELECT * FROM messages WHERE role = 'assistant' AND produced_by_work_id = ?",
    (work_id,),
).fetchall(), "assistant message is absent")
answer = content_text(assistant["content_json"])
lower_answer = answer.lower()
require("ubuntu" in lower_answer and "24.04" in lower_answer,
        "assistant OS/release fact differs")
normalized_arch = lower_answer.replace("-", "_")
require(
    os.environ["HOST_ARCH"].lower() in normalized_arch or "amd64" in lower_answer,
    "assistant architecture fact differs",
)
require(os.environ["HOST_CWD"].lower() in lower_answer, "assistant cwd fact differs")
git_number = os.environ["HOST_GIT"].removeprefix("git version ")
require(
    re.search(r"(?<![0-9.])" + re.escape(git_number) + r"(?![0-9.])", answer)
    is not None,
    "assistant Git fact differs",
)

invocations = connection.execute(
    "SELECT * FROM model_invocations WHERE work_id = ? "
    "ORDER BY agent_step_no, attempt_no", (work_id,)
).fetchall()
require(len(invocations) >= 2, "model did not continue after tool execution")
for invocation in invocations:
    require(invocation["provider_id"] == "openai", "non-OpenAI provider participated")
    require(invocation["provider_model_id"] == "gpt-5.6-luna",
            "non-Luna model participated")
    require(invocation["model_target_id"] == "stage27-openai",
            "non-production target participated")
    require(invocation["target_configuration_version"] == 1,
            "target configuration version differs")
    require(invocation["selection_reason"] == "configured_default",
            "model selection was not the configured default")
    require(invocation["runtime_instance_id"] == os.environ["RUNTIME_ID"],
            "model runtime differs")
    require(invocation["state"] in ("completed", "failed"),
            "model attempt has an unsafe terminal state")
    require(invocation["completed_at"] is not None, "model attempt is not terminal")
    if invocation["state"] == "completed":
        require(invocation["provider_request_id"] is not None,
                "provider request ID is absent")
        require(invocation["provider_response_id"] is not None,
                "provider response ID is absent")

invocation_by_id = {row["model_invocation_id"]: row for row in invocations}
tools = connection.execute(
    "SELECT * FROM tool_executions WHERE work_id = ? "
    "ORDER BY agent_step_no, tool_ordinal", (work_id,)
).fetchall()
require(tools, "the model requested no durable tool")
successful_shells = []
observed_tool_output = []
for tool in tools:
    require(tool["state"] == "completed", "tool execution is not completed")
    require(tool["provider_tool_call_id"] is not None,
            "provider tool-call ID is absent")
    require(tool["runtime_instance_id"] == os.environ["RUNTIME_ID"],
            "tool runtime differs")
    require(tool["workstation_id"] == os.environ["WORKSTATION_ID"],
            "tool workstation differs")
    require(tool["workspace_id"] == os.environ["WORKSPACE_ID"],
            "tool workspace differs")
    source = invocation_by_id.get(tool["source_model_invocation_id"])
    require(source is not None and source["state"] == "completed",
            "tool source model invocation is invalid")
    require(source["tool_call_count"] is not None and source["tool_call_count"] > 0,
            "source model invocation has no tool calls")
    normalized = json.loads(source["normalized_output_json"])
    model_calls = [
        item for item in normalized.get("items", [])
        if item.get("kind") == "tool_call"
        and item.get("call_id") == tool["provider_tool_call_id"]
        and item.get("tool_name") == tool["tool_name"]
        and item.get("arguments_json") == tool["arguments_json"]
    ]
    require(len(model_calls) == 1, "tool request is not present in model output")
    result = json.loads(tool["result_json"])
    observed_tool_output.extend(flatten_strings(result))
    requested_event = exactly_one(connection.execute(
        "SELECT requested.*, cause.event_type AS cause_type, "
        "cause.payload_json AS cause_payload "
        "FROM journal_events requested JOIN journal_events cause "
        "ON cause.event_id = requested.causation_event_id "
        "WHERE requested.work_id = ? "
        "AND requested.event_type = 'tool.execution_requested' "
        "AND json_extract(requested.payload_json, '$.tool_execution_id') = ?",
        (work_id, tool["tool_execution_id"]),
    ).fetchall(), "durable tool request event is absent")
    require(requested_event["cause_type"] == "model.invocation_completed",
            "tool request was not caused by model completion")
    require(
        json.loads(requested_event["cause_payload"]).get("model_invocation_id")
        == tool["source_model_invocation_id"],
        "tool request causation model differs",
    )
    completed_event = exactly_one(connection.execute(
        "SELECT * FROM journal_events WHERE work_id = ? "
        "AND event_type = 'tool.execution_completed' "
        "AND json_extract(payload_json, '$.tool_execution_id') = ?",
        (work_id, tool["tool_execution_id"]),
    ).fetchall(), "durable tool outcome event is absent")
    require(requested_event["journal_offset"] < completed_event["journal_offset"],
            "tool journal ordering differs")
    if tool["tool_name"] == "run_shell":
        require(tool["dispatch_intent_at"] is not None and tool["started_at"] is not None,
                "shell dispatch/start intent is absent")
        require(tool["resolved_cwd"] == os.environ["HOST_CWD"],
                "shell resolved cwd differs")
        require(tool["requested_privilege"] == "user"
                and tool["effective_privilege"] == "user",
                "shell privilege differs")
        require(tool["cleanup_confirmed"] == 1, "shell cleanup is unconfirmed")
        dispatch_event = exactly_one(connection.execute(
            "SELECT * FROM journal_events WHERE work_id = ? "
            "AND event_type = 'tool.execution_dispatching' "
            "AND json_extract(payload_json, '$.tool_execution_id') = ?",
            (work_id, tool["tool_execution_id"]),
        ).fetchall(), "durable shell dispatch event is absent")
        require(
            requested_event["journal_offset"] < dispatch_event["journal_offset"]
            < completed_event["journal_offset"],
            "shell request/dispatch/outcome ordering differs",
        )
        if result.get("result_kind") == "success" and tool["exit_code"] == 0:
            require(tool["timed_out"] == 0 and tool["cancelled"] == 0,
                    "successful shell has unsafe terminal flags")
            require(tool["stdout_artifact_id"] is not None,
                    "successful shell stdout artifact is absent")
            artifact_count = connection.execute(
                "SELECT COUNT(*) FROM artifacts WHERE artifact_id = ? "
                "AND producing_work_id = ? AND producer_kind = 'tool_execution' "
                "AND producer_id = ?",
                (tool["stdout_artifact_id"], work_id, tool["tool_execution_id"]),
            ).fetchone()[0]
            require(artifact_count == 1, "shell stdout artifact provenance differs")
            successful_shells.append(tool)

require(successful_shells, "no successful real shell child was executed")
tool_output = "\n".join(observed_tool_output)
lower_tool_output = tool_output.lower()
require(
    "linux" in lower_tool_output
    or ("ubuntu" in lower_tool_output and "24.04" in lower_tool_output),
    "tool output lacks the OS fact",
)
normalized_tool_arch = lower_tool_output.replace("-", "_")
require(
    os.environ["HOST_ARCH"].lower() in normalized_tool_arch
    or "amd64" in lower_tool_output,
    "tool output lacks the architecture fact",
)
require(os.environ["HOST_CWD"].lower() in lower_tool_output,
        "tool output lacks the cwd fact")
require(
    re.search(r"(?<![0-9.])" + re.escape(git_number) + r"(?![0-9.])", tool_output)
    is not None,
    "tool output lacks the Git fact",
)

final_candidates = [
    row for row in invocations
    if row["state"] == "completed" and row["tool_call_count"] == 0
]
require(final_candidates, "final answering model invocation is absent")
final_invocation = max(final_candidates, key=lambda row: row["agent_step_no"])
require(
    all(tool["agent_step_no"] < final_invocation["agent_step_no"] for tool in tools),
    "final model invocation did not follow every tool",
)
final_output = json.loads(final_invocation["normalized_output_json"])
final_text = "\n".join(
    item.get("text", "")
    for item in final_output.get("items", [])
    if item.get("kind") == "text"
)
require(final_text == answer, "assistant message differs from final model output")
for tool in tools:
    source_count = connection.execute(
        "SELECT COUNT(*) FROM context_manifest_sources "
        "WHERE context_manifest_id = ? "
        "AND source_kind = 'observed_tool_result' "
        "AND source_record_kind = 'tool_execution' AND source_record_id = ?",
        (final_invocation["context_manifest_id"], tool["tool_execution_id"]),
    ).fetchone()[0]
    require(source_count == 1, "final model context omitted a durable tool result")

assistant_event = exactly_one(connection.execute(
    "SELECT assistant.*, cause.event_type AS cause_type, "
    "cause.payload_json AS cause_payload "
    "FROM journal_events assistant JOIN journal_events cause "
    "ON cause.event_id = assistant.causation_event_id "
    "WHERE assistant.work_id = ? "
    "AND assistant.event_type = 'assistant.message_committed'",
    (work_id,),
).fetchall(), "assistant journal event is absent")
require(assistant_event["cause_type"] == "model.invocation_completed",
        "assistant was not caused by model completion")
require(
    json.loads(assistant_event["cause_payload"]).get("model_invocation_id")
    == final_invocation["model_invocation_id"],
    "assistant causation invocation differs",
)
require(
    json.loads(assistant_event["payload_json"]).get("message_id")
    == assistant["message_id"],
    "assistant event message differs",
)
completion_event = exactly_one(connection.execute(
    "SELECT * FROM journal_events WHERE work_id = ? AND event_type = 'work.completed'",
    (work_id,),
).fetchall(), "completed work event is absent")
require(assistant_event["journal_offset"] < completion_event["journal_offset"],
        "assistant/work completion ordering differs")
require(completion_event["causation_event_id"] == assistant_event["event_id"],
        "work completion causation differs")

event_types = [row[0] for row in connection.execute(
    "SELECT event_type FROM journal_events WHERE work_id = ? ORDER BY journal_offset",
    (work_id,),
).fetchall()]
for required_event in (
    "work.queued",
    "work.started",
    "model.invocation_started",
    "model.invocation_completed",
    "tool.execution_requested",
    "tool.execution_dispatching",
    "tool.execution_completed",
    "assistant.message_committed",
    "work.completed",
):
    require(required_event in event_types, f"required durable event is absent: {required_event}")

runtime = exactly_one(connection.execute(
    "SELECT * FROM runtime_instances WHERE runtime_instance_id = ?",
    (os.environ["RUNTIME_ID"],),
).fetchall(), "runtime instance evidence is absent")
require(runtime["state"] == "running", "runtime is not still running")
require(runtime["process_id"] == int(os.environ["MAIN_PID"]), "runtime PID changed")
require(runtime["git_revision"] == os.environ["DEPLOYMENT_COMMIT"],
        "runtime deployment commit changed")

sensitive_patterns = [
    re.compile(r"authorization\s*:\s*bearer\s+\S+", re.IGNORECASE),
    re.compile(r"(?:openai_api_key|credentials_directory)\s*[=:]\s*\S+", re.IGNORECASE),
    re.compile(r"(?<![A-Za-z0-9])sk-[A-Za-z0-9_-]{16,}"),
]
database_text = []
for query in (
    "SELECT content_json FROM messages",
    "SELECT arguments_json FROM tool_executions",
    "SELECT result_json FROM tool_executions",
    "SELECT normalized_output_json FROM model_invocations",
    "SELECT payload_json FROM journal_events WHERE work_id = ?",
):
    parameters = (work_id,) if "?" in query else ()
    database_text.extend(
        row[0] for row in connection.execute(query, parameters).fetchall()
        if row[0] is not None
    )
require(
    not any(pattern.search(value) for pattern in sensitive_patterns for value in database_text),
    "credential-shaped content appears in durable benchmark evidence",
)

journal = subprocess.run(
    [
        "/usr/bin/journalctl",
        "--unit",
        "craxii-server.service",
        "--since",
        f"@{os.environ['JOURNAL_SINCE_EPOCH']}",
        "--output",
        "cat",
        "--no-pager",
    ],
    stdin=subprocess.DEVNULL,
    stdout=subprocess.PIPE,
    stderr=subprocess.DEVNULL,
    check=True,
    text=True,
)
require("provider_credential_unavailable" not in journal.stdout,
        "new provider credential-unavailable telemetry appeared")
require(
    not any(pattern.search(journal.stdout) for pattern in sensitive_patterns),
    "credential-shaped content appears in benchmark telemetry",
)
records = []
for line in journal.stdout.splitlines():
    try:
        record = json.loads(line)
    except json.JSONDecodeError:
        continue
    if isinstance(record, dict):
        records.append(record)

for shell in successful_shells:
    terminal_records = [
        record for record in records
        if record_value(record, "event_name") == "workstation_execution_terminal"
        and record_value(record, "execution_id") == shell["execution_id"]
    ]
    terminal = exactly_one(terminal_records, "workstation terminal telemetry is absent")
    require(record_value(terminal, "result_class") == "exited",
            "workstation telemetry result differs")
    require(record_value(terminal, "cleanup_confirmed") is True,
            "workstation telemetry cleanup is unconfirmed")
    span_names = [span.get("name") for span in record_spans(terminal)]
    for required_span in (
        "work_execution",
        "tool_execution_service",
        "workstation_execute",
    ):
        require(required_span in span_names, f"telemetry path omitted {required_span}")
    work_span = named_span(terminal, "work_execution")
    tool_span = named_span(terminal, "tool_execution_service")
    workstation_span = named_span(terminal, "workstation_execute")
    require(span_value(work_span, "work_id") == work_id, "work span ID differs")
    require(span_value(tool_span, "tool_execution_id") == shell["tool_execution_id"],
            "tool-service span ID differs")
    require(span_value(tool_span, "workstation_execution_id") == shell["execution_id"],
            "tool-service execution ID differs")
    require(span_value(workstation_span, "execution_id") == shell["execution_id"],
            "LocalWorkstation execution ID differs")
    require(span_value(workstation_span, "work_id") == work_id,
            "LocalWorkstation work ID differs")

work_offsets = connection.execute(
    "SELECT MIN(journal_offset), MAX(journal_offset) FROM journal_events WHERE work_id = ?",
    (work_id,),
).fetchone()
model_attempts = ",".join(
    f"{row['model_invocation_id']}:step={row['agent_step_no']}:"
    f"attempt={row['attempt_no']}:state={row['state']}"
    for row in invocations
)
tool_ids = ",".join(row["tool_execution_id"] for row in tools)
execution_ids = ",".join(row["execution_id"] for row in successful_shells)
artifact_count = connection.execute(
    "SELECT COUNT(*) FROM artifacts WHERE producing_work_id = ?", (work_id,)
).fetchone()[0]

print(f"STAGE27_CONVERSATION_ID={os.environ['CONVERSATION_ID']}")
print(f"STAGE27_ACCEPTED_MESSAGE_ID={message_id}")
print(f"STAGE27_WORK_ID={work_id}")
print(f"STAGE27_CORRELATION_ID={correlation_id}")
print(f"STAGE27_MODEL_INVOCATION_ATTEMPTS={model_attempts}")
print(f"STAGE27_TOOL_EXECUTION_IDS={tool_ids}")
print(f"STAGE27_WORKSTATION_EXECUTION_IDS={execution_ids}")
print(f"STAGE27_ASSISTANT_MESSAGE_ID={assistant['message_id']}")
print(f"STAGE27_COMMAND_COMMITTED_CURSOR={command['committed_cursor']}")
print(f"STAGE27_MESSAGE_ACCEPTED_CURSOR={trigger['journal_offset']}")
print(f"STAGE27_ASSISTANT_CURSOR={assistant_event['journal_offset']}")
print(f"STAGE27_WORK_COMPLETED_CURSOR={completion_event['journal_offset']}")
print(f"STAGE27_WORK_JOURNAL_RANGE={work_offsets[0]}..{work_offsets[1]}")
print(f"STAGE27_DURABLE_ARTIFACT_COUNT={artifact_count}")
print("STAGE27_DURABLE_USER_MESSAGE_COUNT=1")
print("STAGE27_DURABLE_WORK_ITEM_COUNT=1")
print("STAGE27_AGENT_RUNTIME=YES")
print("STAGE27_TOOL_EXECUTION_SERVICE=YES")
print("STAGE27_TOOL_REGISTRY=YES")
print("STAGE27_LOCAL_WORKSTATION=YES")
print("STAGE27_MODEL_CONTINUATION=YES")
print("STAGE27_TOOL_RESULT_IN_MODEL_CONTINUATION=YES")
print("STAGE27_BACKEND_USER=craxii-server")
print("STAGE27_REAL_LUNA_PROVIDER=openai")
print("STAGE27_REAL_LUNA_MODEL=gpt-5.6-luna")
print("STAGE27_REAL_LUNA_FALLBACK=NO")
print("STAGE27_REAL_LUNA_MODEL_TOOL_REQUEST=YES")
print(f"STAGE27_TOOL_EXECUTION_UID={os.environ['CHILD_UID']}")
print(f"STAGE27_TOOL_EXECUTION_USER={os.environ['CHILD_USER']}")
print(f"STAGE27_TOOL_EXECUTION_CWD={os.environ['HOST_CWD']}")
print("STAGE27_TOOL_PATH_PRODUCTION_LAUNCHER=YES")
print("STAGE27_PRODUCTION_DIRECT_SEAM_BYPASS=NO")
print("STAGE27_CHILD_NOT_ROOT=YES")
print("STAGE27_CHILD_NOT_BACKEND=YES")
print("STAGE27_CHILD_PROVIDER_CREDENTIAL_INHERITED=NO")
print("STAGE27_CHILD_CREDENTIALS_DIRECTORY_INHERITED=NO")
print(f"STAGE27_EXECUTION_CGROUP_ROOT={os.environ['CGROUP_ROOT']}")
print("STAGE27_EXECUTION_CGROUP_SUPERVISED=YES")
print(f"STAGE27_INDEPENDENT_OS={os.environ['HOST_OS']}")
print(f"STAGE27_INDEPENDENT_ARCH={os.environ['HOST_ARCH']}")
print(f"STAGE27_INDEPENDENT_CWD={os.environ['HOST_CWD']}")
print(f"STAGE27_INDEPENDENT_GIT={os.environ['HOST_GIT']}")
print("STAGE27_ASSISTANT_FACTS_MATCH_HOST=YES")
print("STAGE27_DURABLE_TOOL_OUTCOME=YES")
print("STAGE27_WORK_TERMINAL_STATE=completed")
print("SERVICE_STATE=active")
print(f"MAIN_PID={os.environ['MAIN_PID']}")
print("SERVICE_RESTARTS_UNCHANGED=YES")
print(f"HEALTH_LIVE_HTTP={os.environ['HEALTH_LIVE']}")
print(f"HEALTH_READY_HTTP={os.environ['HEALTH_READY']}")
print("CURRENT_RUN_PROVIDER_CREDENTIAL_UNAVAILABLE=NO")
print("STAGE27_CREDENTIAL_SHAPED_LEAKAGE=NO")
print("STAGE27_REAL_LUNA_CANONICAL_BENCHMARK: PASSED")
PY
STAGE27_LUNA_BENCHMARK
```
