#!/usr/bin/env python3
"""Independent Stage 26 evidence inspection; receives exact secrets only over stdin."""

from __future__ import annotations

import argparse
import base64
import hashlib
import json
import os
import pathlib
import sqlite3
import subprocess
import sys


PROMPTS = [
    "Inspect your machine and tell me what OS, CPU architecture, current directory, and Git version you have.",
    "What Git version did you find?",
]


def content_text(encoded: str) -> str:
    value = json.loads(encoded)
    if isinstance(value, dict):
        value = value.get("blocks", [])
    return "\n".join(
        block.get("text", "") for block in value
        if isinstance(block, dict) and block.get("type") == "text"
    )


def scan(root: pathlib.Path, secrets: list[bytes]) -> int:
    paths = [root] if root.is_file() else root.rglob("*")
    count = 0
    for path in paths:
        if path.is_symlink() or not path.is_file():
            continue
        data = path.read_bytes()
        count += 1
        if any(secret and secret in data for secret in secrets):
            raise RuntimeError("independent exact-secret scan found a match")
    return count


def secure_write(path: pathlib.Path, value: object) -> None:
    data = json.dumps(value, indent=2, sort_keys=True).encode("utf-8")
    descriptor = os.open(path, os.O_WRONLY | os.O_CREAT | os.O_EXCL, 0o600)
    with os.fdopen(descriptor, "wb") as stream:
        stream.write(data)


def xcresult_has_passed_test(path: pathlib.Path, test_name: str) -> bool:
    result = subprocess.run(
        ["/usr/bin/xcrun", "xcresulttool", "get", "test-results", "tests", "--path",
         str(path)],
        stdin=subprocess.DEVNULL, stdout=subprocess.PIPE, stderr=subprocess.PIPE, check=False,
    )
    if result.returncode != 0:
        return False
    try:
        document = json.loads(result.stdout)
    except (json.JSONDecodeError, UnicodeDecodeError):
        return False

    def passed(value: object) -> bool:
        if isinstance(value, dict):
            name = value.get("name")
            if (
                value.get("nodeType") == "Test Case"
                and name in {test_name, f"{test_name}()"}
                and value.get("result") == "Passed"
            ):
                return True
            return any(passed(child) for child in value.values())
        if isinstance(value, list):
            return any(passed(child) for child in value)
        return False

    return passed(document)


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--report", type=pathlib.Path, required=True)
    parser.add_argument("--verification", type=pathlib.Path, required=True)
    args = parser.parse_args()
    secret_input = json.load(sys.stdin)
    secrets = [base64.b64decode(value) for value in secret_input]
    report = json.loads(args.report.read_text(encoding="utf-8"))
    runtime = pathlib.Path(report["configuration"]["runtime_root"])
    database = runtime / "state/db/craxii.sqlite3"
    connection = sqlite3.connect(f"file:{database}?mode=ro", uri=True)
    connection.row_factory = sqlite3.Row

    users = connection.execute(
        "SELECT message_id, client_message_id, content_json FROM messages "
        "WHERE role = 'user' ORDER BY committed_at"
    ).fetchall()
    if [content_text(row["content_json"]) for row in users] != PROMPTS:
        raise RuntimeError("independent durable prompt sequence diverged")
    works = connection.execute(
        "SELECT work_id, state, terminal_reason_code FROM work_items ORDER BY conversation_work_ordinal"
    ).fetchall()
    if len(works) != 2 or any(row["state"] != "completed" for row in works):
        raise RuntimeError("independent durable work state diverged")
    for index, work in enumerate(works):
        providers = connection.execute(
            "SELECT DISTINCT provider_id, provider_model_id FROM model_invocations WHERE work_id = ?",
            (work["work_id"],),
        ).fetchall()
        if not providers or any(tuple(row) != ("openai", "gpt-5.6-luna") for row in providers):
            raise RuntimeError("independent model identity check failed")
        tool_count = connection.execute(
            "SELECT COUNT(*) FROM tool_executions WHERE work_id = ?", (work["work_id"],)
        ).fetchone()[0]
        if (index == 0 and tool_count < 1) or (index == 1 and tool_count != 0):
            raise RuntimeError("independent tool-count check failed")
    first_tools = connection.execute(
        "SELECT state, provider_tool_call_id, workstation_id, workspace_id, result_json "
        "FROM tool_executions WHERE work_id = ?", (works[0]["work_id"],)
    ).fetchall()
    if any(row["state"] != "completed" or row["provider_tool_call_id"] is None for row in first_tools):
        raise RuntimeError("independent workstation provenance check failed")
    assistants = connection.execute(
        "SELECT message_id, produced_by_work_id, content_json FROM messages "
        "WHERE role = 'assistant' ORDER BY committed_at"
    ).fetchall()
    if len(assistants) != 2:
        raise RuntimeError("independent assistant cardinality check failed")
    durable_hashes = {
        row["message_id"]: hashlib.sha256(content_text(row["content_json"]).encode()).hexdigest()
        for row in assistants
    }
    observations = {
        row["phase"]: row for row in report["ui_durable_correlation"]["observations"]
    }
    for phase, assistant in zip(["first_turn", "follow_up"], assistants, strict=True):
        observed = observations.get(phase)
        if observed is None or observed["row_identifier"] != f"transcript.row.{assistant['message_id']}":
            raise RuntimeError("independent native row identity check failed")
        if observed["text_sha256"] != durable_hashes[assistant["message_id"]]:
            raise RuntimeError("independent native/durable digest check failed")
    restart = report["restart_replay"]
    if not (restart["saved_cursor"] < restart["replay_through_cursor"]):
        raise RuntimeError("independent replay high-water check failed")
    if restart["first_runtime_id"] == restart["second_runtime_id"]:
        raise RuntimeError("independent runtime-restart check failed")
    for turn_name in ["first_turn", "follow_up"]:
        turn = report[turn_name]
        if not turn.get("request_id") or turn["message_id"] != turn["user_message_id"]:
            raise RuntimeError("independent client-command/request/message correlation failed")
    recovery = connection.execute(
        "SELECT COUNT(*) FROM journal_events WHERE runtime_instance_id = ? "
        "AND event_type = 'runtime.recovery_performed'", (restart["second_runtime_id"],)
    ).fetchone()[0]
    if recovery != 1:
        raise RuntimeError("independent recovery-before-readiness evidence missing")
    forbidden = connection.execute(
        "SELECT COUNT(*) FROM model_invocations WHERE provider_id <> 'openai' "
        "OR provider_model_id <> 'gpt-5.6-luna'"
    ).fetchone()[0]
    connection.close()
    if forbidden:
        raise RuntimeError("independent fixture/provider exclusion failed")

    cancellation = report["deterministic_native_cancellation"]
    if not (
        cancellation["status"] == "passed"
        and cancellation["terminal_state"] == "cancelled"
        and cancellation["provider_spend"] is False
        and cancellation["relaunch_projection_remained_cancelled"] is True
    ):
        raise RuntimeError("independent cancellation contract failed")

    xcresult = pathlib.Path(report["build_artifacts"]["xcresult"])
    if not xcresult_has_passed_test(
        xcresult, "testStage26LiveNativeLunaRestartRelaunchAndFollowUp"
    ):
        raise RuntimeError("independent XCResult inspection did not find a passed native test")
    cancellation_xcresult = pathlib.Path(cancellation["xcresult"])
    if not xcresult_has_passed_test(
        cancellation_xcresult, "testStage26DeterministicNativeCancellationSmoke"
    ):
        raise RuntimeError("independent cancellation XCResult inspection failed")

    scanned = sum(scan(path, secrets) for path in [
        runtime, xcresult, cancellation_xcresult, args.report,
        pathlib.Path(cancellation["evidence_root"]),
    ])
    secure_write(args.verification, {
        "status": "passed",
        "durable_backend_inspected": True,
        "native_xcresults_inspected": True,
        "provider_model_verified": "openai/gpt-5.6-luna",
        "tool_and_workstation_provenance_verified": True,
        "replay_restart_relaunch_verified": True,
        "follow_up_zero_tools_verified": True,
        "deterministic_native_cancellation_verified": True,
        "fixture_absence_verified": True,
        "exact_secret_scan_verified": True,
        "scanned_file_count": scanned,
    })
    return 0


if __name__ == "__main__":
    sys.exit(main())
