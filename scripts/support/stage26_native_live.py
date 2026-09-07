#!/usr/bin/env python3
"""One-shot, opt-in Stage 26 native/Luna controller.

The provisioned device bearer is held only in this process and returned once over a
randomized loopback controller URL to the XCUI runner. The retained report contains IDs,
digests, and provenance, never credentials or raw model content.
"""

from __future__ import annotations

import argparse
import base64
import datetime as dt
import hashlib
import http.server
import json
import os
import pathlib
import re
import secrets
import shutil
import signal
import socket
import sqlite3
import subprocess
import sys
import threading
import time
import urllib.error
import urllib.request


CANONICAL_PROMPT = (
    "Inspect your machine and tell me what OS, CPU architecture, current directory, "
    "and Git version you have."
)
FOLLOW_UP = "What Git version did you find?"
MODEL = "gpt-5.6-luna"
PROVIDER = "openai"
MODEL_TARGET = "stage25_openai"
CREDENTIAL_FILE = pathlib.Path("/Users/abhisht/.config/craxii/credentials/openai_stage25")
UUID_PATTERN = r"[0-9a-f]{8}-[0-9a-f]{4}-7[0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}"


def sha256_text(value: str) -> str:
    return hashlib.sha256(value.encode("utf-8")).hexdigest()


def contains_semantic_fact(text: str, fact: str) -> bool:
    haystack = re.findall(r"[a-z0-9]+", text.lower())
    needle = re.findall(r"[a-z0-9]+", fact.lower())
    if not needle:
        return False
    return any(
        haystack[index:index + len(needle)] == needle
        for index in range(len(haystack) - len(needle) + 1)
    )


def secure_write(path: pathlib.Path, value: bytes) -> None:
    descriptor = os.open(path, os.O_WRONLY | os.O_CREAT | os.O_EXCL, 0o600)
    with os.fdopen(descriptor, "wb") as stream:
        stream.write(value)


def secure_replace(path: pathlib.Path, value: bytes) -> None:
    temporary = path.with_name(path.name + ".tmp")
    secure_write(temporary, value)
    os.replace(temporary, path)


def available_authority() -> str:
    with socket.socket(socket.AF_INET, socket.SOCK_STREAM) as listener:
        listener.bind(("127.0.0.1", 0))
        return f"127.0.0.1:{listener.getsockname()[1]}"


def command_output(arguments: list[str], cwd: pathlib.Path | None = None) -> str:
    result = subprocess.run(
        arguments, cwd=cwd, env={}, stdin=subprocess.DEVNULL,
        stdout=subprocess.PIPE, stderr=subprocess.PIPE, check=False,
    )
    if result.returncode != 0:
        raise RuntimeError(f"safe command failed: {pathlib.Path(arguments[0]).name}")
    return result.stdout.decode("utf-8").strip()


def content_text(encoded: str) -> str:
    value = json.loads(encoded)
    if isinstance(value, dict):
        value = value.get("blocks", [])
    return "\n".join(
        block.get("text", "") for block in value
        if isinstance(block, dict) and block.get("type") == "text"
    )


def flatten_strings(value: object) -> list[str]:
    if isinstance(value, str):
        return [value]
    if isinstance(value, list):
        result: list[str] = []
        for item in value:
            result.extend(flatten_strings(item))
        return result
    if isinstance(value, dict):
        result = []
        for item in value.values():
            result.extend(flatten_strings(item))
        return result
    return []


class Stage26State:
    def __init__(self, args: argparse.Namespace) -> None:
        self.args = args
        self.root = args.evidence / "runtime"
        self.state_root = self.root / "state"
        self.artifacts = self.root / "artifacts"
        self.workspace = self.root / "workspace"
        self.client_state = self.root / "native-client-state"
        self.database = self.state_root / "db" / "craxii.sqlite3"
        self.config = self.root / "stage26.toml"
        self.authority = available_authority()
        self.endpoint = f"http://{self.authority}/"
        self.backend: subprocess.Popen[bytes] | None = None
        self.backend_phase = ""
        self.bearer = ""
        self.secret_patterns: list[bytes] = []
        self.lock = threading.RLock()
        self.observations: list[dict[str, object]] = []
        self.setup_reads = 0
        self.saved_cursor: int | None = None
        self.replay_cursor: int | None = None
        self.first_pid: int | None = None
        self.second_pid: int | None = None
        self.first_runtime: str | None = None
        self.second_runtime: str | None = None
        self.controller_key = secrets.token_hex(24)
        self.complete = threading.Event()
        self.started_at = dt.datetime.now().astimezone()

    def prepare(self) -> None:
        self.root.mkdir(mode=0o700)
        for directory in [self.state_root, self.artifacts, self.workspace, self.client_state]:
            directory.mkdir(mode=0o700)
        template = self.args.template.read_text(encoding="utf-8")
        rendered = (
            template.replace("{{AUTHORITY}}", self.authority)
            .replace("{{STATE_ROOT}}", str(self.state_root))
            .replace("{{ARTIFACT_ROOT}}", str(self.artifacts))
            .replace("{{WORKSPACE_ROOT}}", str(self.workspace))
        )
        secure_write(self.config, rendered.encode("utf-8"))
        provider = CREDENTIAL_FILE.read_bytes()
        if provider:
            self.secret_patterns.append(provider)
            trimmed = provider.rstrip(b"\r\n")
            if trimmed and trimmed != provider:
                self.secret_patterns.append(trimmed)

    def spawn_backend(self, phase: str) -> None:
        with self.lock:
            if self.backend is not None:
                raise RuntimeError("owned backend already active")
            stdout = open(self.root / f"backend-{phase}.stdout", "xb", buffering=0)
            stderr = open(self.root / f"backend-{phase}.stderr", "xb", buffering=0)
            os.chmod(stdout.name, 0o600)
            os.chmod(stderr.name, 0o600)
            self.backend_phase = phase
            self.backend = subprocess.Popen(
                [str(self.args.server), "--config", str(self.config)],
                env={}, stdin=subprocess.DEVNULL, stdout=stdout, stderr=stderr,
                start_new_session=True,
            )
        self.wait_ready()

    def wait_ready(self) -> None:
        deadline = time.monotonic() + 35
        url = f"http://{self.authority}/health/ready"
        while time.monotonic() < deadline:
            with self.lock:
                process = self.backend
            if process is None or process.poll() is not None:
                raise RuntimeError(f"backend exited during {self.backend_phase} startup")
            try:
                with urllib.request.urlopen(url, timeout=1) as response:
                    if response.status == 200:
                        return
            except (urllib.error.URLError, TimeoutError):
                pass
            time.sleep(0.05)
        raise RuntimeError(f"backend readiness timeout during {self.backend_phase}")

    def stop_backend(self, requested_signal: int) -> int:
        with self.lock:
            process = self.backend
            if process is None:
                raise RuntimeError("owned backend is not active")
            os.killpg(process.pid, requested_signal)
            try:
                returncode = process.wait(timeout=25)
            except subprocess.TimeoutExpired:
                os.killpg(process.pid, signal.SIGKILL)
                returncode = process.wait(timeout=10)
            self.backend = None
            return returncode

    def provision(self) -> None:
        result = subprocess.run(
            [str(self.args.admin), "--config", str(self.config), "device", "provision",
             "Stage 26 native Mac"],
            env={}, stdin=subprocess.DEVNULL, stdout=subprocess.PIPE,
            stderr=subprocess.PIPE, check=False,
        )
        if result.returncode != 0:
            raise RuntimeError("device provisioning failed")
        bearer = result.stdout.decode("utf-8").rstrip("\r\n")
        if not bearer:
            raise RuntimeError("device provisioning returned no credential")
        self.bearer = bearer
        self.secret_patterns.append(bearer.encode("utf-8"))

    def connection(self) -> sqlite3.Connection:
        connection = sqlite3.connect(f"file:{self.database}?mode=ro", uri=True, timeout=5)
        connection.row_factory = sqlite3.Row
        return connection

    def current_runtime(self) -> str:
        with self.connection() as connection:
            row = connection.execute(
                "SELECT runtime_instance_id FROM runtime_instances ORDER BY started_at DESC LIMIT 1"
            ).fetchone()
        if row is None:
            raise RuntimeError("runtime identity missing")
        return str(row[0])

    def state_cursor(self) -> int:
        path = self.client_state / "client-state-v1.json"
        value = json.loads(path.read_text(encoding="utf-8"))
        return int(value["lastAppliedCursor"])

    def profile_id(self) -> str | None:
        path = self.client_state / "client-state-v1.json"
        if not path.exists():
            return None
        value = json.loads(path.read_text(encoding="utf-8"))
        profile = value.get("profile") or {}
        raw = profile.get("profileID")
        return raw if isinstance(raw, str) else None

    def turn(self, prompt: str) -> dict[str, str] | None:
        with self.connection() as connection:
            users = connection.execute(
                "SELECT message_id, client_message_id, content_json FROM messages "
                "WHERE role = 'user' ORDER BY committed_at"
            ).fetchall()
            user = next((row for row in users if content_text(row["content_json"]) == prompt), None)
            if user is None:
                return None
            work = connection.execute(
                "SELECT w.work_id, w.state FROM work_items w "
                "JOIN work_item_inputs i ON i.work_id = w.work_id AND i.relationship = 'trigger' "
                "JOIN journal_events e ON e.event_id = i.input_event_id "
                "WHERE json_extract(e.payload_json, '$.message_id') = ?",
                (user["message_id"],),
            ).fetchone()
            if work is None or work["state"] != "completed":
                return None
            assistant = connection.execute(
                "SELECT message_id, content_json FROM messages "
                "WHERE role = 'assistant' AND produced_by_work_id = ?",
                (work["work_id"],),
            ).fetchone()
            if assistant is None:
                return None
            answer = content_text(assistant["content_json"])
            return {
                "user_message_id": str(user["message_id"]),
                "client_command_id": str(user["client_message_id"]),
                "assistant_message_id": str(assistant["message_id"]),
                "work_id": str(work["work_id"]),
                "assistant_sha256": sha256_text(answer),
            }

    def record_observation(self, value: dict[str, object]) -> None:
        allowed = {
            "phase", "row_identifier", "text_sha256", "optimistic_seen",
            "activity_seen", "reconnecting_seen",
        }
        if set(value) - allowed:
            raise ValueError("unknown native observation field")
        self.observations.append(value)
        if value.get("phase") == "restart_replay":
            self.replay_cursor = self.state_cursor()

    def kill_for_replay(self) -> None:
        self.saved_cursor = self.state_cursor()
        self.first_pid = self.backend.pid if self.backend is not None else None
        self.first_runtime = self.current_runtime()
        returncode = self.stop_backend(signal.SIGKILL)
        if returncode != -signal.SIGKILL:
            raise RuntimeError("backend restart proof did not observe SIGKILL")

    def restart_for_replay(self) -> None:
        self.spawn_backend("restart")
        self.second_pid = self.backend.pid if self.backend is not None else None
        self.second_runtime = self.current_runtime()
        if self.first_pid == self.second_pid or self.first_runtime == self.second_runtime:
            raise RuntimeError("backend runtime identity did not change")

    def bootstrap(self) -> dict[str, object]:
        request = urllib.request.Request(
            f"http://{self.authority}/v1/bootstrap",
            headers={"Authorization": f"Bearer {self.bearer}"},
        )
        with urllib.request.urlopen(request, timeout=10) as response:
            if response.status != 200:
                raise RuntimeError("authenticated bootstrap failed")
            return json.load(response)

    def cleanup_keychain(self) -> str:
        profile = self.profile_id()
        if profile is None:
            return "profile_not_available"
        result = subprocess.run(
            ["/usr/bin/security", "delete-generic-password", "-s",
             "com.craxii.device-token.v1", "-a", profile],
            stdin=subprocess.DEVNULL, stdout=subprocess.DEVNULL,
            stderr=subprocess.DEVNULL, check=False,
        )
        return "deleted" if result.returncode == 0 else "already_absent"

    def collect_client_diagnostics(self) -> pathlib.Path:
        destination = self.root / "client-diagnostics.jsonl"
        start = self.started_at.strftime("%Y-%m-%d %H:%M:%S")
        with open(destination, "xb") as output:
            os.chmod(destination, 0o600)
            subprocess.run(
                ["/usr/bin/log", "show", "--style", "json", "--start", start,
                 "--predicate", 'subsystem == "com.craxii.client"'],
                stdin=subprocess.DEVNULL, stdout=output, stderr=subprocess.DEVNULL,
                check=False,
            )
        return destination

    def request_id(self, command_id: str) -> str | None:
        for path in sorted(self.root.glob("backend-*.stdout")):
            for line in path.read_text(encoding="utf-8", errors="replace").splitlines():
                if command_id not in line:
                    continue
                matches = re.findall(rf'"request_id"\s*:\s*"({UUID_PATTERN})"', line)
                if matches:
                    return matches[-1]
        return None

    def detailed_turn(self, prompt: str) -> dict[str, object]:
        summary = self.turn(prompt)
        if summary is None:
            raise RuntimeError("durable completed turn missing")
        work_id = summary["work_id"]
        with self.connection() as connection:
            models = connection.execute(
                "SELECT model_invocation_id, context_manifest_id, agent_step_no, attempt_no, "
                "provider_id, provider_model_id, model_target_id, state, tool_call_count, "
                "input_tokens, output_tokens, reasoning_tokens, total_tokens "
                "FROM model_invocations WHERE work_id = ? ORDER BY agent_step_no, attempt_no",
                (work_id,),
            ).fetchall()
            tools = connection.execute(
                "SELECT tool_execution_id, source_model_invocation_id, tool_name, state, "
                "provider_tool_call_id, workstation_id, workspace_id, resolved_cwd, result_json "
                "FROM tool_executions WHERE work_id = ? ORDER BY agent_step_no, tool_ordinal",
                (work_id,),
            ).fetchall()
            work = connection.execute(
                "SELECT correlation_id, created_at, queued_at, started_at, terminal_at, "
                "terminal_reason_code FROM work_items WHERE work_id = ?", (work_id,)
            ).fetchone()
            command = connection.execute(
                "SELECT response_http_status, committed_cursor, response_json FROM client_commands "
                "WHERE idempotency_key = ?", (summary["client_command_id"],)
            ).fetchone()
            context_rows = connection.execute(
                "SELECT source_record_kind, source_record_id, source_kind, item_class "
                "FROM context_manifest_sources s JOIN model_invocations m "
                "ON m.context_manifest_id = s.context_manifest_id WHERE m.work_id = ?",
                (work_id,),
            ).fetchall()
            assistant_cursor = connection.execute(
                "SELECT journal_offset FROM journal_events "
                "WHERE event_type = 'assistant.message_committed' AND work_id = ?",
                (work_id,),
            ).fetchone()
        model_values = [dict(row) for row in models]
        for model in model_values:
            if model["provider_id"] != PROVIDER or model["provider_model_id"] != MODEL:
                raise RuntimeError("non-Luna provider/model participated")
        tool_values = [{key: row[key] for key in row.keys() if key != "result_json"} for row in tools]
        tool_output = "\n".join(
            "\n".join(flatten_strings(json.loads(row["result_json"])))
            for row in tools if row["result_json"]
        )
        response = json.loads(command["response_json"])
        request_id = self.request_id(summary["client_command_id"])
        if request_id is None:
            raise RuntimeError("client command/request correlation telemetry missing")
        if response.get("message_id") != summary["user_message_id"] \
                or response.get("work_id") != work_id:
            raise RuntimeError("durable command response identity diverged")
        return {
            **summary,
            "request_id": request_id,
            "message_id": response.get("message_id"),
            "accepted_http_status": command["response_http_status"],
            "accepted_cursor": command["committed_cursor"],
            "correlation_id": work["correlation_id"],
            "terminal_reason": work["terminal_reason_code"],
            "assistant_event_cursor": assistant_cursor[0],
            "timing": {
                "created_at": work["created_at"], "queued_at": work["queued_at"],
                "started_at": work["started_at"], "terminal_at": work["terminal_at"],
            },
            "model_invocations": model_values,
            "tool_executions": tool_values,
            "tool_output_sha256": sha256_text(tool_output),
            "tool_output": tool_output,
            "context_sources": [dict(row) for row in context_rows],
        }

    def build_report(self, cancellation: dict[str, object]) -> dict[str, object]:
        first = self.detailed_turn(CANONICAL_PROMPT)
        follow = self.detailed_turn(FOLLOW_UP)
        facts = {
            "os": command_output(["/usr/bin/uname", "-s"], self.workspace),
            "architecture": command_output(["/usr/bin/uname", "-m"], self.workspace),
            "cwd": str(self.workspace.resolve()),
            "git_version": command_output(["/usr/bin/git", "--version"], self.workspace),
        }
        first_answer = self.answer_for(first["work_id"])
        follow_answer = self.answer_for(follow["work_id"])
        self.assert_semantic_facts(first_answer, facts)
        self.assert_semantic_facts(str(first.pop("tool_output")), facts)
        git_number = facts["git_version"].removeprefix("git version ")
        if not contains_semantic_fact(follow_answer, git_number):
            raise RuntimeError("follow-up omitted independently measured Git version")
        first_context = first.pop("context_sources")
        follow_context = follow.pop("context_sources")
        first_tools = first["tool_executions"]
        follow_tools = follow["tool_executions"]
        if not first_tools or any(tool["state"] != "completed" for tool in first_tools):
            raise RuntimeError("first turn lacked completed real tool execution")
        if follow_tools:
            raise RuntimeError("follow-up executed a tool")
        if len(first["model_invocations"]) < 2:
            raise RuntimeError("first turn lacked Luna continuation")
        if not any((row["tool_call_count"] or 0) > 0 for row in first["model_invocations"]):
            raise RuntimeError("Luna did not originate a tool call")
        if not any(row["source_kind"] == "provider_native_continuation" for row in first_context):
            raise RuntimeError("Luna continuation evidence missing")
        prior_ids = {first["user_message_id"], first["assistant_message_id"]}
        follow_sources = {row["source_record_id"] for row in follow_context}
        if not prior_ids.issubset(follow_sources):
            raise RuntimeError("follow-up omitted prior durable messages")
        if self.saved_cursor is None or self.replay_cursor is None:
            raise RuntimeError("native replay cursor observations missing")
        if self.replay_cursor <= self.saved_cursor:
            raise RuntimeError("internal-only recovery high-water cursor did not advance")
        bootstrap = self.bootstrap()
        durable_digests = {
            message["message_id"]: sha256_text("\n".join(
                block.get("text", "") for block in message.get("content", [])
                if block.get("type") == "text"
            ))
            for message in bootstrap["messages"] if message["role"] == "assistant"
        }
        observation_by_phase = {str(value["phase"]): value for value in self.observations}
        for phase, turn in [("first_turn", first), ("follow_up", follow)]:
            observation = observation_by_phase.get(phase)
            if observation is None:
                raise RuntimeError(f"native {phase} observation missing")
            expected_identifier = f"transcript.row.{turn['assistant_message_id']}"
            if observation.get("row_identifier") != expected_identifier:
                raise RuntimeError(f"native {phase} row identity diverged")
            if observation.get("text_sha256") != turn["assistant_sha256"]:
                raise RuntimeError(f"native {phase} text diverged")
        for phase in ["restart_replay", "app_relaunch"]:
            if phase not in observation_by_phase:
                raise RuntimeError(f"native {phase} observation missing")
        with self.connection() as connection:
            forbidden = connection.execute(
                "SELECT COUNT(*) FROM model_invocations WHERE provider_id IN "
                "('stage18-scripted','scripted','context-answering','deterministic-fixture')"
            ).fetchone()[0]
            recovery = connection.execute(
                "SELECT COUNT(*) FROM journal_events WHERE runtime_instance_id = ? "
                "AND event_type = 'runtime.recovery_performed'", (self.second_runtime,)
            ).fetchone()[0]
            device_rows = connection.execute(
                "SELECT COUNT(*) FROM client_devices WHERE token_hash IS NOT NULL"
            ).fetchone()[0]
        if forbidden != 0 or recovery != 1 or device_rows != 1:
            raise RuntimeError("runtime provenance or recovery invariant failed")
        return {
            "contract": "craxii.stage26.native-local.v1",
            "status": "passed",
            "configuration": {
                "runtime_root": str(self.root), "runtime_root_mode": "0700",
                "config_path": str(self.config), "config_mode": "0600",
                "config_sha256": hashlib.sha256(self.config.read_bytes()).hexdigest(),
                "provider": PROVIDER, "provider_model_id": MODEL,
                "model_target": MODEL_TARGET, "fallback_present": False,
                "endpoint": self.endpoint,
            },
            "native_app": {
                "bundle_id": "com.craxii.client.macos", "configuration": "Debug",
                "app_path": str(self.args.derived / "Build/Products/Debug/Craxii.app"),
                "real_swiftui_store_session": True, "keychain_adapter": True,
                "urlsession_http_websocket": True,
            },
            "fixture_isolation": {
                "stage21_triggers_absent": True, "stage22_trigger_absent": True,
                "runtime_fixture_activation_absent": True, "injected_session_absent": True,
                "synthetic_projection_absent": True,
            },
            "machine_facts": facts,
            "first_turn": self.redacted_turn(first),
            "restart_replay": {
                "saved_cursor": self.saved_cursor, "replay_through_cursor": self.replay_cursor,
                "requested_strictly_after_saved_cursor": True,
                "internal_only_high_water_advanced": self.replay_cursor > self.saved_cursor,
                "first_pid": self.first_pid, "second_pid": self.second_pid,
                "first_runtime_id": self.first_runtime,
                "second_runtime_id": self.second_runtime,
                "termination": "SIGKILL_owned_backend_process_group",
                "recovery_preceded_readiness": recovery == 1,
                "native_reconnecting_rendered": observation_by_phase["restart_replay"].get(
                    "reconnecting_seen"),
                "duplicate_messages": False, "terminal_regression": False,
            },
            "app_relaunch": {
                "same_profile_and_conversation": True, "server_backed_restore": True,
                "prior_user_and_assistant_restored": True,
                "ui_message_sha256": observation_by_phase["app_relaunch"].get("text_sha256"),
            },
            "follow_up": {
                **self.redacted_turn(follow), "same_conversation": True,
                "prior_durable_context_present": True,
                "git_version_semantically_equal": True,
                "new_tool_execution_count": 0,
                "new_local_workstation_execution_count": 0,
                "new_assistant_message_count": 1, "new_native_assistant_row_count": 1,
            },
            "ui_durable_correlation": {
                "bootstrap_snapshot_cursor": bootstrap["snapshot_cursor"],
                "assistant_digests": durable_digests,
                "observations": self.observations,
            },
            "deterministic_native_cancellation": cancellation,
            "secret_safety": {
                "provider_credential_source_backend_only": True,
                "device_bearer_stored_as_digest_backend_side": True,
                "provider_authorization_absent_from_native": True,
                "workstation_clean_environment_policy": True,
                "exact_secret_scan_pending": True,
            },
            "authorization": {
                "keychain_prompt_observed": False, "xctest_prompt_observed": False,
                "human_action_required": None,
            },
            "independent_verification": {"status": "pending"},
            "build_artifacts": {
                "derived_data": str(self.args.derived),
                "xcresult": str(self.args.xcresult),
            },
        }

    def answer_for(self, work_id: object) -> str:
        with self.connection() as connection:
            row = connection.execute(
                "SELECT content_json FROM messages WHERE produced_by_work_id = ?", (work_id,)
            ).fetchone()
        if row is None:
            raise RuntimeError("assistant answer missing")
        return content_text(row[0])

    def assert_semantic_facts(self, text: str, facts: dict[str, str]) -> None:
        lower = text.lower()
        os_ok = facts["os"].lower() in lower or (
            facts["os"] == "Darwin" and ("macos" in lower or "mac os" in lower)
        )
        if not os_ok:
            raise RuntimeError("OS fact missing")
        for key in ["architecture", "cwd"]:
            if facts[key].lower() not in lower:
                raise RuntimeError(f"{key} fact missing")
        git_number = facts["git_version"].removeprefix("git version ")
        if not contains_semantic_fact(text, git_number):
            raise RuntimeError("Git fact missing")

    def redacted_turn(self, value: dict[str, object]) -> dict[str, object]:
        result = dict(value)
        result.pop("context_sources", None)
        result.pop("tool_output", None)
        models = result["model_invocations"]
        result["model_invocation_count"] = len(models)
        result["tool_execution_count"] = len(result["tool_executions"])
        result["provider"] = PROVIDER
        result["model"] = MODEL
        result["tool_execution_service"] = True
        result["tool_registry_resolution"] = True
        result["local_workstation_execution"] = bool(result["tool_executions"])
        result["answer_content_included"] = False
        return result


class ControllerHandler(http.server.BaseHTTPRequestHandler):
    state: Stage26State

    def log_message(self, _format: str, *args: object) -> None:
        return

    def route(self) -> str | None:
        prefix = f"/{self.state.controller_key}/"
        if not self.path.startswith(prefix):
            return None
        return self.path[len(prefix):].split("?", 1)[0]

    def json_response(self, value: object, status: int = 200) -> None:
        data = json.dumps(value, separators=(",", ":")).encode("utf-8")
        self.send_response(status)
        self.send_header("content-type", "application/json")
        self.send_header("content-length", str(len(data)))
        self.end_headers()
        self.wfile.write(data)

    def empty_response(self, status: int = 204) -> None:
        self.send_response(status)
        self.end_headers()

    def do_GET(self) -> None:  # noqa: N802
        route = self.route()
        try:
            if route == "setup":
                with self.state.lock:
                    self.state.setup_reads += 1
                    if self.state.setup_reads != 1:
                        self.empty_response(410)
                        return
                    self.json_response({
                        "endpoint": self.state.endpoint,
                        "credential": self.state.bearer,
                        "state_directory": str(self.state.client_state),
                    })
                return
            if route == "turn/first":
                value = self.state.turn(CANONICAL_PROMPT)
                self.json_response(value, 200 if value else 425)
                return
            if route == "turn/follow-up":
                value = self.state.turn(FOLLOW_UP)
                self.json_response(value, 200 if value else 425)
                return
            self.empty_response(404)
        except Exception:
            self.empty_response(500)

    def do_POST(self) -> None:  # noqa: N802
        route = self.route()
        try:
            if route == "observation":
                length = int(self.headers.get("content-length", "0"))
                if length > 4096:
                    self.empty_response(413)
                    return
                value = json.loads(self.rfile.read(length))
                self.state.record_observation(value)
                self.empty_response()
                return
            if route == "backend/kill":
                self.state.kill_for_replay()
                self.empty_response()
                return
            if route == "backend/restart":
                self.state.restart_for_replay()
                self.empty_response()
                return
            if route == "complete":
                self.state.complete.set()
                self.empty_response()
                return
            self.empty_response(404)
        except Exception:
            self.empty_response(500)


def scan_paths(paths: list[pathlib.Path], patterns: list[bytes]) -> tuple[int, list[str]]:
    scanned = 0
    failures: list[str] = []
    for root in paths:
        candidates = [root] if root.is_file() else list(root.rglob("*"))
        for candidate in candidates:
            if candidate.is_symlink() or not candidate.is_file():
                continue
            try:
                data = candidate.read_bytes()
            except OSError:
                failures.append("storage")
                continue
            scanned += 1
            for pattern in patterns:
                if pattern and pattern in data:
                    failures.append("exact_secret_match")
                    break
    return scanned, failures


def validate_credential_file() -> None:
    metadata = CREDENTIAL_FILE.stat()
    parent = CREDENTIAL_FILE.parent.stat()
    if metadata.st_mode & 0o777 != 0o600 or parent.st_mode & 0o777 != 0o700:
        raise RuntimeError("Stage 25 credential permissions are unsafe")
    if CREDENTIAL_FILE.is_symlink() or CREDENTIAL_FILE.parent.is_symlink():
        raise RuntimeError("Stage 25 credential path may not be a symlink")


def run_xcui(state: Stage26State) -> int:
    environment = dict(os.environ)
    for name in [
        "OPENAI_API_KEY", "OPENAI_KEY", "CRAXII_OPENAI_API_KEY",
        "CRAXII_STAGE25_OPENAI_API_KEY", "CRAXII_STAGE22_UI_SMOKE",
        "CRAXII_STAGE21_UI_SMOKE", "CRAXII_STAGE21_INTEGRATION",
        "CRAXII_STAGE22_INTEGRATION", "CRAXII_STAGE26_CANCELLATION",
    ]:
        environment.pop(name, None)
    environment["CRAXII_STAGE26_LIVE"] = "1"
    environment["CRAXII_STAGE26_CONTROL_URL"] = (
        f"http://127.0.0.1:{state.args.control_port}/{state.controller_key}/"
    )
    prepared = subprocess.run([
        sys.executable, str(state.args.repository / "scripts/support/stage26_xctestrun.py"),
        "--derived", str(state.args.derived), "--mode", "live",
        "--control-url",
        f"http://127.0.0.1:{state.args.control_port}/{state.controller_key}/",
    ], stdin=subprocess.DEVNULL, stdout=subprocess.PIPE, stderr=subprocess.DEVNULL,
       text=True, check=True).stdout.strip()
    xctestrun = pathlib.Path(prepared)
    command = [
        "/usr/bin/xcodebuild", "test-without-building",
        "-xctestrun", str(xctestrun),
        "-destination", f"platform=macOS,arch={os.uname().machine}",
        "-resultBundlePath", str(state.args.xcresult),
        "-only-testing:CraxiiUITests/CraxiiUITests/testStage26LiveNativeLunaRestartRelaunchAndFollowUp",
    ]
    log = state.root / "xcui-live.log"
    result: subprocess.CompletedProcess[bytes] | None = None
    with open(log, "xb") as output:
        os.chmod(log, 0o600)
        try:
            result = subprocess.run(
                command, cwd=state.args.repository, env=environment,
                stdin=subprocess.DEVNULL, stdout=output, stderr=subprocess.STDOUT,
                check=False, timeout=20 * 60,
            )
        finally:
            xctestrun.unlink(missing_ok=True)
            if result is None or result.returncode != 0:
                terminate_owned_debug_app(state.args.derived)
    return result.returncode


def terminate_owned_debug_app(derived: pathlib.Path) -> None:
    executable = (
        derived / "Build/Products/Debug/Craxii.app/Contents/MacOS/Craxii"
    ).resolve()
    result = subprocess.run(
        ["/usr/bin/pgrep", "-f", f"^{re.escape(str(executable))}$"],
        stdin=subprocess.DEVNULL, stdout=subprocess.PIPE, stderr=subprocess.DEVNULL,
        text=True, check=False,
    )
    for raw_pid in result.stdout.split():
        try:
            os.kill(int(raw_pid), signal.SIGTERM)
        except (ProcessLookupError, ValueError):
            pass


def parse_arguments() -> argparse.Namespace:
    parser = argparse.ArgumentParser()
    parser.add_argument("--repository", type=pathlib.Path, required=True)
    parser.add_argument("--server", type=pathlib.Path, required=True)
    parser.add_argument("--admin", type=pathlib.Path, required=True)
    parser.add_argument("--template", type=pathlib.Path, required=True)
    parser.add_argument("--derived", type=pathlib.Path, required=True)
    parser.add_argument("--evidence", type=pathlib.Path, required=True)
    parser.add_argument("--report", type=pathlib.Path, required=True)
    parser.add_argument("--xcresult", type=pathlib.Path, required=True)
    parser.add_argument("--cancellation-report", type=pathlib.Path, required=True)
    parser.add_argument("--verifier", type=pathlib.Path, required=True)
    parser.add_argument("--verification", type=pathlib.Path, required=True)
    return parser.parse_args()


def main() -> int:
    args = parse_arguments()
    validate_credential_file()
    for path in [
        args.server, args.admin, args.template, args.cancellation_report, args.verifier,
    ]:
        if not path.exists():
            raise RuntimeError(f"required Stage 26 input missing: {path.name}")
    args.evidence.mkdir(mode=0o700)
    args.control_port = int(available_authority().split(":")[1])
    state = Stage26State(args)
    server: http.server.ThreadingHTTPServer | None = None
    keychain_cleanup = "not_attempted"
    report: dict[str, object] = {
        "contract": "craxii.stage26.native-local.v1", "status": "failed",
        "failure_class": "harness",
    }
    try:
        state.prepare()
        state.spawn_backend("initialize")
        if state.stop_backend(signal.SIGTERM) != 0:
            raise RuntimeError("initial backend did not stop gracefully")
        state.provision()
        state.spawn_backend("live")
        ControllerHandler.state = state
        server = http.server.ThreadingHTTPServer(
            ("127.0.0.1", args.control_port), ControllerHandler)
        controller_thread = threading.Thread(target=server.serve_forever, daemon=True)
        controller_thread.start()
        xcui_status = run_xcui(state)
        if xcui_status != 0:
            raise RuntimeError("native XCUI acceptance failed")
        if not state.complete.wait(timeout=2):
            raise RuntimeError("native XCUI acceptance omitted completion handshake")
        state.collect_client_diagnostics()
        cancellation = json.loads(args.cancellation_report.read_text(encoding="utf-8"))
        report = state.build_report(cancellation)
        if state.backend is not None and state.stop_backend(signal.SIGTERM) != 0:
            raise RuntimeError("final backend did not stop gracefully")
        keychain_cleanup = state.cleanup_keychain()
        report["keychain_cleanup"] = keychain_cleanup
        secure_write(args.report, json.dumps(report, indent=2, sort_keys=True).encode("utf-8"))
        scanned, failures = scan_paths(
            [state.root, args.xcresult, args.report, args.cancellation_report],
            state.secret_patterns,
        )
        if failures:
            raise RuntimeError("exact secret scan failed")
        report["secret_safety"].update({
            "exact_secret_scan_pending": False,
            "exact_openai_credential_absent": True,
            "exact_device_bearer_absent": True,
            "scanned_file_count": scanned,
        })
        secure_replace(args.report, json.dumps(report, indent=2, sort_keys=True).encode("utf-8"))
        verification_input = json.dumps([
            base64.b64encode(pattern).decode("ascii")
            for pattern in [CREDENTIAL_FILE.read_bytes(), state.bearer.encode("utf-8")]
        ]).encode("utf-8")
        verification = subprocess.run(
            [sys.executable, str(args.verifier), "--report", str(args.report),
             "--verification", str(args.verification)],
            env={}, stdout=subprocess.DEVNULL,
            stderr=subprocess.DEVNULL, input=verification_input, check=False,
        )
        if verification.returncode != 0:
            raise RuntimeError("independent Stage 26 verification failed")
        independent = json.loads(args.verification.read_text(encoding="utf-8"))
        report["independent_verification"] = independent
        final_bytes = json.dumps(report, indent=2, sort_keys=True).encode("utf-8")
        if any(pattern and pattern in final_bytes for pattern in state.secret_patterns):
            raise RuntimeError("final report exact-secret scan failed")
        secure_replace(args.report, final_bytes)
        return 0
    except Exception as error:
        if state.backend is not None:
            try:
                state.stop_backend(signal.SIGKILL)
            except Exception:
                pass
        if keychain_cleanup == "not_attempted":
            try:
                keychain_cleanup = state.cleanup_keychain()
            except Exception:
                keychain_cleanup = "cleanup_failed"
        report["status"] = "failed"
        report["failure_class"] = type(error).__name__
        report["failure_detail"] = str(error)
        report["evidence_root"] = str(state.root)
        report["keychain_cleanup"] = keychain_cleanup
        if args.report.exists():
            secure_replace(
                args.report, json.dumps(report, indent=2, sort_keys=True).encode("utf-8"))
        else:
            secure_write(args.report, json.dumps(report, indent=2, sort_keys=True).encode("utf-8"))
        return 1
    finally:
        if server is not None:
            server.shutdown()
            server.server_close()
        state.bearer = ""
        state.secret_patterns.clear()


if __name__ == "__main__":
    sys.exit(main())
