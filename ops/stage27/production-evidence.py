#!/usr/bin/env python3
"""Capture and compare redacted Stage 27 production-host evidence."""

from __future__ import annotations

import argparse
import datetime as dt
import grp
import hashlib
import http.client
import json
import os
import pathlib
import pwd
import sqlite3
import stat
import subprocess
import sys
import tempfile
from typing import Any


DATABASE = pathlib.Path("/var/lib/craxii/db/craxii.sqlite3")
ARTIFACT_ROOT = pathlib.Path("/var/lib/craxii/artifacts")
WORKSPACE_SENTINEL = pathlib.Path(
    "/srv/craxii/workspaces/primary/.craxii-stage27-persistence-sentinel-v1"
)
EVIDENCE_SENTINEL = pathlib.Path(
    "/srv/craxii-data/stage27-evidence/persistence-sentinel-v1"
)
CREDENTIAL = pathlib.Path("/etc/craxii/credentials/openai_provider")
CREDENTIAL_DIRECTORY = CREDENTIAL.parent
CURRENT = pathlib.Path("/opt/craxii/current")
SERVICE = "craxii-server.service"
CGROUP_ROOT = pathlib.Path(
    "/sys/fs/cgroup/system.slice/craxii-server.service/craxii-executions"
)
DATA_MOUNT = pathlib.Path("/srv/craxii-data")
BIND_MOUNTS = {
    "/var/lib/craxii": "/state",
    "/srv/craxii/workspaces": "/workspaces",
    "/home/craxii": "/home/craxii",
}
WORKSPACE_SENTINEL_ACCESS_ACL = {
    "user:": "rw-",
    "user:craxii-server": "r-x",
    "group:": "---",
    "mask:": "r--",
    "other:": "---",
}
STABLE_TABLES = (
    "craxii_principals",
    "workstations",
    "workspaces",
    "conversations",
    "client_devices",
    "messages",
    "work_items",
    "model_invocations",
    "tool_executions",
    "artifacts",
    "context_manifests",
)
RELEASE_FILES = {
    "craxii-server": ("root", "craxii-server", "0550"),
    "craxii-admin": ("root", "craxii-server", "0550"),
    "craxii-stage27-luna-benchmark": ("root", "craxii-server", "0550"),
    "craxii-workstation-launcher": ("root", "craxii-server", "4750"),
    "craxii-workstation-reader": ("root", "root", "0111"),
}


class EvidenceError(RuntimeError):
    pass


def require(condition: bool, message: str) -> None:
    if not condition:
        raise EvidenceError(message)


def run(arguments: list[str], *, allowed_statuses: tuple[int, ...] = (0,)) -> str:
    completed = subprocess.run(
        arguments,
        stdin=subprocess.DEVNULL,
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
        check=False,
        text=True,
        timeout=15,
    )
    if completed.returncode not in allowed_statuses:
        raise EvidenceError(
            f"host command failed without retained output: {pathlib.Path(arguments[0]).name}"
        )
    return completed.stdout.strip()


def systemd_properties() -> dict[str, str]:
    names = [
        "ActiveState",
        "SubState",
        "UnitFileState",
        "User",
        "Group",
        "MainPID",
        "ControlGroup",
        "Delegate",
        "KillMode",
        "Restart",
        "RestartUSec",
        "TimeoutStopUSec",
        "UMask",
        "LimitNOFILE",
        "LimitNOFILESoft",
        "LimitNPROC",
        "LimitNPROCSoft",
        "LimitCORE",
        "LimitCORESoft",
        "AmbientCapabilities",
        "CapabilityBoundingSet",
        "WorkingDirectory",
        "FragmentPath",
        "DropInPaths",
        "NeedDaemonReload",
        "Environment",
        "KillSignal",
        "ExecMainStartTimestampMonotonic",
    ]
    output = run(
        ["/usr/bin/systemctl", "show", SERVICE]
        + [item for name in names for item in ("--property", name)]
    )
    properties = dict(line.split("=", 1) for line in output.splitlines() if "=" in line)
    require(set(names) <= properties.keys(), "systemd property set is incomplete")
    return properties


def health(path: str) -> dict[str, Any]:
    connection = http.client.HTTPConnection("127.0.0.1", 8080, timeout=5)
    try:
        connection.request("GET", path, headers={"Host": "127.0.0.1:8080"})
        response = connection.getresponse()
        body = response.read(65536)
    finally:
        connection.close()
    require(len(body) < 65536, f"oversized health response for {path}")
    payload = json.loads(body)
    return {"status": response.status, "state": payload.get("status")}


def mount_record(path: pathlib.Path) -> dict[str, str]:
    output = run(
        [
            "/usr/bin/findmnt",
            "-nro",
            "SOURCE,FSTYPE,FSROOT,OPTIONS",
            "--target",
            str(path),
        ]
    )
    fields = output.split(maxsplit=3)
    require(len(fields) == 4, f"unexpected mount record for {path}")
    return {
        "target": str(path),
        "source": fields[0],
        "fstype": fields[1],
        "fsroot": fields[2],
        "options": fields[3],
    }


def file_record(path: pathlib.Path, *, include_hash: bool) -> dict[str, Any]:
    metadata = path.lstat()
    require(stat.S_ISREG(metadata.st_mode), f"expected regular file: {path}")
    require(not path.is_symlink(), f"symbolic link refused: {path}")
    record: dict[str, Any] = {
        "path": str(path),
        "owner": pwd.getpwuid(metadata.st_uid).pw_name,
        "group": grp.getgrgid(metadata.st_gid).gr_name,
        "mode": f"{stat.S_IMODE(metadata.st_mode):04o}",
        "bytes": metadata.st_size,
    }
    if include_hash:
        digest = hashlib.sha256()
        with path.open("rb") as handle:
            for chunk in iter(lambda: handle.read(1024 * 1024), b""):
                digest.update(chunk)
        record["sha256"] = digest.hexdigest()
    return record


def release_record(path: pathlib.Path, expected_commit: str) -> dict[str, Any]:
    manifest_path = path / ".craxii-stage27-build-manifest"
    manifest = file_record(manifest_path, include_hash=True)
    require(
        (manifest["owner"], manifest["group"], manifest["mode"])
        == ("root", "root", "0444"),
        "installed build manifest metadata mismatch",
    )
    lines = manifest_path.read_text(encoding="ascii").splitlines()
    require(len(lines) == len(RELEASE_FILES) + 1, "installed build manifest entry count mismatch")
    require(lines[0] == f"commit={expected_commit}", "installed build manifest commit mismatch")
    expected_entries: dict[str, str] = {}
    for line in lines[1:]:
        fields = line.split("  ", 1)
        require(
            len(fields) == 2
            and len(fields[0]) == 64
            and not (set(fields[0]) - set("0123456789abcdef"))
            and fields[1] in RELEASE_FILES
            and fields[1] not in expected_entries,
            "installed build manifest contains an invalid entry",
        )
        expected_entries[fields[1]] = fields[0]
    require(set(expected_entries) == set(RELEASE_FILES), "installed build manifest file set mismatch")

    files: dict[str, dict[str, Any]] = {}
    for name, expected_metadata in RELEASE_FILES.items():
        record = file_record(path / name, include_hash=True)
        require(
            (record["owner"], record["group"], record["mode"]) == expected_metadata,
            f"installed release metadata mismatch: {name}",
        )
        require(record["sha256"] == expected_entries[name], f"installed release digest mismatch: {name}")
        files[name] = record
    return {"manifest": manifest, "files": files}


def parse_posix_access_acl(value: str) -> dict[str, str]:
    entries: dict[str, str] = {}
    for raw_line in value.splitlines():
        line = raw_line.split("#", 1)[0].strip()
        if not line:
            continue
        fields = line.split(":")
        require(len(fields) == 3, "workspace sentinel ACL contains a malformed entry")
        kind, qualifier, permissions = fields
        require(
            kind in {"user", "group", "mask", "other"},
            "workspace sentinel ACL contains an unexpected entry kind",
        )
        require(
            len(permissions) == 3
            and permissions[0] in {"r", "-"}
            and permissions[1] in {"w", "-"}
            and permissions[2] in {"x", "-"},
            "workspace sentinel ACL contains invalid permissions",
        )
        key = f"{kind}:{qualifier}"
        require(key not in entries, "workspace sentinel ACL contains a duplicate entry")
        entries[key] = permissions
    return entries


def masked_permissions(permissions: str, mask: str) -> str:
    return "".join(
        permission if permission != "-" and mask[index] != "-" else "-"
        for index, permission in enumerate(permissions)
    )


def permission_digit(permissions: str) -> int:
    return sum(
        bit for permission, bit in zip(permissions, (4, 2, 1)) if permission != "-"
    )


def validate_workspace_sentinel_acl(
    owner: str, group: str, mode: str, value: str
) -> dict[str, Any]:
    require(owner == "craxii", "workspace sentinel owner mismatch")
    require(group == "craxii", "workspace sentinel group mismatch")
    entries = parse_posix_access_acl(value)
    require(
        entries == WORKSPACE_SENTINEL_ACCESS_ACL,
        "workspace sentinel access ACL differs from the inherited production policy",
    )
    effective_server = masked_permissions(
        entries["user:craxii-server"], entries["mask:"]
    )
    expected_mode = "0" + "".join(
        str(permission_digit(entries[key])) for key in ("user:", "mask:", "other:")
    )
    require(mode == expected_mode, "workspace sentinel mode does not reflect its access ACL")
    require(entries["user:"] == "rw-", "workspace sentinel owner access mismatch")
    require(effective_server == "r--", "workspace sentinel server access is not read-only")
    require(entries["other:"] == "---", "workspace sentinel other access is not empty")
    return {
        "entries": entries,
        "effective_owner": entries["user:"],
        "effective_craxii_server": effective_server,
        "effective_other": entries["other:"],
        "mode": mode,
    }


def workspace_sentinel_record() -> dict[str, Any]:
    record = file_record(WORKSPACE_SENTINEL, include_hash=True)
    acl = run(
        [
            "/usr/bin/getfacl",
            "--absolute-names",
            "--omit-header",
            str(WORKSPACE_SENTINEL),
        ]
    )
    record["access_acl"] = validate_workspace_sentinel_acl(
        record["owner"], record["group"], record["mode"], acl
    )
    return record


def credential_record(main_pid: int) -> dict[str, Any]:
    metadata = CREDENTIAL.lstat()
    directory = CREDENTIAL_DIRECTORY.lstat()
    require(stat.S_ISREG(metadata.st_mode), "provider credential is not a regular file")
    require(not CREDENTIAL.is_symlink(), "provider credential is a symbolic link")
    require(stat.S_ISDIR(directory.st_mode), "provider credential directory is invalid")
    require(not CREDENTIAL_DIRECTORY.is_symlink(), "provider credential directory is a symlink")

    workstation_access = subprocess.run(
        ["/usr/sbin/runuser", "-u", "craxii", "--", "/usr/bin/test", "-r", str(CREDENTIAL)],
        stdin=subprocess.DEVNULL,
        stdout=subprocess.DEVNULL,
        stderr=subprocess.DEVNULL,
        check=False,
        timeout=5,
    )
    require(workstation_access.returncode != 0, "workstation identity can read provider credential")

    server = pwd.getpwnam("craxii-server")
    probe = (
        f"test ! -r {CREDENTIAL}; "
        "test -z \"${CREDENTIALS_DIRECTORY-}\"; "
        "test -z \"${OPENAI_API_KEY-}\"; "
        f"test ! -r /proc/{main_pid}/environ"
    )
    launcher_probe = subprocess.run(
        [
            "/usr/bin/setpriv",
            f"--reuid={server.pw_uid}",
            f"--regid={server.pw_gid}",
            "--clear-groups",
            "/usr/bin/env",
            "-i",
            str(CURRENT / "craxii-workstation-launcher"),
            "shell",
            "00000000-0000-7000-8000-000000000000",
            "00000000-0000-7000-8000-000000000000",
            probe,
        ],
        cwd="/srv/craxii/workspaces/primary",
        stdin=subprocess.DEVNULL,
        stdout=subprocess.DEVNULL,
        stderr=subprocess.DEVNULL,
        check=False,
        timeout=5,
    )
    require(launcher_probe.returncode == 0, "model-child credential boundary probe failed")

    return {
        "present": True,
        "path": str(CREDENTIAL),
        "owner": pwd.getpwuid(metadata.st_uid).pw_name,
        "group": grp.getgrgid(metadata.st_gid).gr_name,
        "mode": f"{stat.S_IMODE(metadata.st_mode):04o}",
        "directory_owner": pwd.getpwuid(directory.st_uid).pw_name,
        "directory_group": grp.getgrgid(directory.st_gid).gr_name,
        "directory_mode": f"{stat.S_IMODE(directory.st_mode):04o}",
        "workstation_readable": False,
        "model_child_boundary": "pass",
        "content_or_hash_inspected": False,
    }


def rows_by_state(connection: sqlite3.Connection, table: str) -> dict[str, int]:
    return {
        str(state): int(count)
        for state, count in connection.execute(
            f"SELECT state, COUNT(*) FROM {table} GROUP BY state ORDER BY state"
        )
    }


def database_record() -> dict[str, Any]:
    require(DATABASE.is_file() and not DATABASE.is_symlink(), "canonical database is absent or unsafe")
    uri = f"file:{DATABASE}?mode=ro"
    connection = sqlite3.connect(uri, uri=True, timeout=5)
    connection.row_factory = sqlite3.Row
    try:
        connection.execute("PRAGMA query_only = ON")
        connection.execute("BEGIN")
        quick_check = connection.execute("PRAGMA quick_check").fetchone()[0]
        journal_mode = str(connection.execute("PRAGMA journal_mode").fetchone()[0])
        foreign_key_violations = connection.execute(
            "SELECT COUNT(*) FROM pragma_foreign_key_check"
        ).fetchone()[0]
        applied_schema = connection.execute(
            "SELECT MAX(version) FROM _sqlx_migrations WHERE success = 1"
        ).fetchone()[0]
        identity = connection.execute(
            "SELECT craxii_id FROM craxii_principals ORDER BY craxii_id"
        ).fetchall()
        require(len(identity) == 1, "canonical Craxii identity is not singular")
        running = connection.execute(
            "SELECT * FROM runtime_instances WHERE state = 'running' ORDER BY started_at DESC"
        ).fetchall()
        require(len(running) == 1, "canonical runtime ownership is not singular")
        current = dict(running[0])
        history = [
            dict(row)
            for row in connection.execute(
                "SELECT runtime_instance_id, linux_boot_id, process_id, git_revision, "
                "schema_version, state, started_at, stopped_at, stop_reason "
                "FROM runtime_instances ORDER BY started_at DESC, runtime_instance_id DESC LIMIT 10"
            )
        ]
        recovery_rows = connection.execute(
            "SELECT journal_offset, payload_json FROM journal_events "
            "WHERE runtime_instance_id = ? AND event_type = 'runtime.recovery_performed' "
            "ORDER BY journal_offset",
            (current["runtime_instance_id"],),
        ).fetchall()
        require(len(recovery_rows) == 1, "current runtime recovery evidence is not singular")
        recovery = json.loads(recovery_rows[0]["payload_json"])
        counts = {
            table: int(connection.execute(f"SELECT COUNT(*) FROM {table}").fetchone()[0])
            for table in STABLE_TABLES
        }
        states = {
            "work_items": rows_by_state(connection, "work_items"),
            "model_invocations": rows_by_state(connection, "model_invocations"),
            "tool_executions": rows_by_state(connection, "tool_executions"),
        }
        ambiguity = {
            "active_work": int(
                connection.execute(
                    "SELECT COUNT(*) FROM work_items WHERE state IN "
                    "('queued','running','waiting_on_model','waiting_on_tool','cancel_requested')"
                ).fetchone()[0]
            ),
            "interrupted_work": int(
                connection.execute(
                    "SELECT COUNT(*) FROM work_items WHERE state = 'interrupted'"
                ).fetchone()[0]
            ),
            "model_outcome_unknown": int(
                connection.execute(
                    "SELECT COUNT(*) FROM model_invocations WHERE state = 'provider_outcome_unknown'"
                ).fetchone()[0]
            ),
            "tool_outcome_unknown": int(
                connection.execute(
                    "SELECT COUNT(*) FROM tool_executions WHERE state = 'outcome_unknown'"
                ).fetchone()[0]
            ),
        }
        terminal_evidence_violations = int(
            connection.execute(
                "SELECT COUNT(*) FROM tool_executions WHERE "
                "(state <> 'completed' AND (timed_out IS NOT NULL OR cancelled IS NOT NULL)) OR "
                "(state = 'completed' AND dispatch_intent_at IS NULL AND "
                " (timed_out IS NOT NULL OR cancelled IS NOT NULL)) OR "
                "(state = 'completed' AND dispatch_intent_at IS NOT NULL AND ("
                " timed_out IS NULL OR cancelled IS NULL OR "
                " json_extract(result_json, '$.result_kind') IS NULL OR "
                " json_extract(result_json, '$.result_kind') NOT IN ("
                "  'success','validation_rejection','unknown_tool','authority_denial',"
                "  'file_error','process_exit','signal_termination','timeout','cancellation',"
                "  'spawn_failure','cleanup_failure') OR "
                " (json_extract(result_json, '$.result_kind') = 'timeout' AND "
                "  (timed_out <> 1 OR cancelled <> 0)) OR "
                " (json_extract(result_json, '$.result_kind') = 'cancellation' AND "
                "  (timed_out <> 0 OR cancelled <> 1)) OR "
                " (json_extract(result_json, '$.result_kind') NOT IN ('timeout','cancellation') AND "
                "  (timed_out <> 0 OR cancelled <> 0))))"
            ).fetchone()[0]
        )
        terminal_current_attempt_violations = int(
            connection.execute(
                "SELECT COUNT(*) FROM work_items WHERE state IN "
                "('completed','failed','cancelled','interrupted') AND "
                "(current_model_invocation_id IS NOT NULL OR current_tool_execution_id IS NOT NULL)"
            ).fetchone()[0]
        )
        latest_artifact = connection.execute(
            "SELECT artifact_id, storage_key, sha256, captured_byte_count, retention_class, "
            "created_at FROM artifacts ORDER BY created_at DESC, artifact_id DESC LIMIT 1"
        ).fetchone()
        max_journal_offset = int(
            connection.execute("SELECT COALESCE(MAX(journal_offset), 0) FROM journal_events").fetchone()[0]
        )
    finally:
        if connection.in_transaction:
            connection.rollback()
        connection.close()

    artifact: dict[str, Any] | None = None
    if latest_artifact is not None:
        artifact = dict(latest_artifact)
        artifact_root = ARTIFACT_ROOT.resolve(strict=True)
        artifact_path = (artifact_root / artifact["storage_key"]).resolve(strict=True)
        require(
            artifact_path.is_relative_to(artifact_root),
            "artifact storage key escapes the canonical artifact root",
        )
        file_metadata = file_record(artifact_path, include_hash=True)
        require(file_metadata["sha256"] == artifact["sha256"], "artifact digest mismatch")
        require(file_metadata["bytes"] == artifact["captured_byte_count"], "artifact size mismatch")
        artifact["file"] = file_metadata

    database_file = file_record(DATABASE, include_hash=False)
    return {
        "path": str(DATABASE),
        "file": database_file,
        "quick_check": quick_check,
        "journal_mode": journal_mode,
        "foreign_key_violations": int(foreign_key_violations),
        "applied_schema_version": int(applied_schema),
        "craxii_id": identity[0][0],
        "current_runtime": current,
        "runtime_history": history,
        "current_recovery": {
            "journal_offset": int(recovery_rows[0]["journal_offset"]),
            "payload": recovery,
        },
        "stable_counts": counts,
        "states": states,
        "ambiguity": ambiguity,
        "terminal_evidence_violations": terminal_evidence_violations,
        "terminal_current_attempt_violations": terminal_current_attempt_violations,
        "max_journal_offset": max_journal_offset,
        "artifact_sentinel": artifact,
    }


def cgroup_record(main_pid: int, control_group: str) -> dict[str, Any]:
    require(control_group == "/system.slice/craxii-server.service", "unexpected service cgroup")
    process_cgroup = pathlib.Path(f"/proc/{main_pid}/cgroup").read_text(encoding="utf-8")
    require(
        f"0::{control_group}" in process_cgroup.splitlines(),
        "backend process is outside the systemd service cgroup",
    )
    require(CGROUP_ROOT.is_dir(), "delegated execution cgroup root is absent")
    child_directories = sorted(
        str(path.relative_to(CGROUP_ROOT))
        for path in CGROUP_ROOT.iterdir()
        if path.is_dir()
    )
    execution_pids: set[int] = set()
    for procs in CGROUP_ROOT.rglob("cgroup.procs"):
        for value in procs.read_text(encoding="ascii").split():
            execution_pids.add(int(value))
    return {
        "service_control_group": control_group,
        "main_process_membership": "pass",
        "delegated_root": str(CGROUP_ROOT),
        "execution_child_directories": child_directories,
        "execution_process_count": len(execution_pids),
        "leak_status": "clean" if not child_directories and not execution_pids else "residue",
    }


def capture(arguments: argparse.Namespace) -> dict[str, Any]:
    require(os.geteuid() == 0, "run evidence capture as root on the authorized Stage 27 host")
    require(sys.platform.startswith("linux"), "production evidence capture requires Linux")
    properties = systemd_properties()
    require(properties["ActiveState"] == "active", "service is not active")
    require(properties["SubState"] == "running", "service is not running")
    require(properties["UnitFileState"] == "enabled", "service is not enabled")
    require(properties["User"] == "craxii-server", "service user mismatch")
    require(properties["Group"] == "craxii-server", "service group mismatch")
    require(properties["Delegate"] == "yes", "service cgroup delegation is disabled")
    require(properties["KillMode"] == "control-group", "service KillMode mismatch")
    require(properties["Restart"] == "on-failure", "service restart policy mismatch")
    require(properties["RestartUSec"] == "2s", "service restart delay mismatch")
    require(properties["TimeoutStopUSec"] == "30s", "service stop timeout mismatch")
    require(properties["UMask"] == "0077", "service umask mismatch")
    require(properties["LimitNOFILE"] == "65536", "service NOFILE limit mismatch")
    require(properties["LimitNOFILESoft"] == "65536", "service soft NOFILE limit mismatch")
    require(properties["LimitNPROC"] == "16384", "service NPROC limit mismatch")
    require(properties["LimitNPROCSoft"] == "16384", "service soft NPROC limit mismatch")
    require(properties["LimitCORE"] == "0", "service core limit mismatch")
    require(properties["LimitCORESoft"] == "0", "service soft core limit mismatch")
    require(set(properties["AmbientCapabilities"].split()) == {"cap_kill"}, "service ambient capability mismatch")
    require(
        set(properties["CapabilityBoundingSet"].split())
        == {"cap_kill", "cap_setgid", "cap_setuid", "cap_setpcap"},
        "service capability bounding set mismatch",
    )
    require(properties["WorkingDirectory"] == "/var/lib/craxii", "service working directory mismatch")
    require(properties["FragmentPath"] == "/etc/systemd/system/craxii-server.service", "service fragment path mismatch")
    require(properties["DropInPaths"] == "", "unexpected service drop-in configuration")
    require(properties["NeedDaemonReload"] == "no", "systemd manager state is stale")
    require(properties["Environment"] == "", "unexpected explicit service environment")
    require(properties["KillSignal"] == "15", "service termination signal mismatch")
    main_pid = int(properties["MainPID"])
    require(main_pid > 0 and pathlib.Path(f"/proc/{main_pid}").is_dir(), "service MainPID is invalid")
    process_owner = pwd.getpwuid(pathlib.Path(f"/proc/{main_pid}").stat().st_uid).pw_name
    process_status = pathlib.Path(f"/proc/{main_pid}/status").read_text(encoding="utf-8")
    process_gid = int(
        process_status.split("Gid:\t", 1)[1].splitlines()[0].split()[0]
    )
    process_group = grp.getgrgid(process_gid).gr_name
    require(process_owner == "craxii-server", "backend process owner mismatch")
    require(process_group == "craxii-server", "backend process group mismatch")
    status_fields = {
        line.split(":", 1)[0]: line.split(":", 1)[1].strip()
        for line in process_status.splitlines()
        if ":" in line
    }
    server = pwd.getpwnam("craxii-server")
    require(
        [int(value) for value in status_fields["Uid"].split()] == [server.pw_uid] * 4,
        "backend real/effective/saved/filesystem UID mismatch",
    )
    require(
        [int(value) for value in status_fields["Gid"].split()] == [server.pw_gid] * 4,
        "backend real/effective/saved/filesystem GID mismatch",
    )
    require(
        set(int(value) for value in status_fields["Groups"].split()) <= {server.pw_gid},
        "backend has an unexpected supplementary group",
    )
    kill_capability = 1 << 5
    transition_bounding_set = sum(1 << capability for capability in (5, 6, 7, 8))
    for field in ("CapInh", "CapPrm", "CapEff", "CapAmb"):
        require(int(status_fields[field], 16) == kill_capability, f"backend {field} mismatch")
    require(
        int(status_fields["CapBnd"], 16) == transition_bounding_set,
        "backend capability bounding set mismatch",
    )
    require(status_fields["NoNewPrivs"] == "0", "backend cannot execute the setuid launcher")
    require(status_fields["Umask"] == "0077", "backend process umask mismatch")
    process_cwd = str(pathlib.Path(f"/proc/{main_pid}/cwd").resolve(strict=True))
    require(process_cwd == "/var/lib/craxii", "backend process cwd mismatch")
    process_environment = pathlib.Path(f"/proc/{main_pid}/environ").read_bytes()
    environment_names = sorted(
        entry.split(b"=", 1)[0].decode("ascii")
        for entry in process_environment.split(b"\0")
        if entry and b"=" in entry
    )
    for forbidden in (
        "OPENAI_API_KEY",
        "AWS_ACCESS_KEY_ID",
        "AWS_SECRET_ACCESS_KEY",
        "AWS_SESSION_TOKEN",
        "AWS_PROFILE",
        "AWS_SHARED_CREDENTIALS_FILE",
        "AWS_WEB_IDENTITY_TOKEN_FILE",
        "AWS_CONTAINER_CREDENTIALS_RELATIVE_URI",
        "AWS_CONTAINER_CREDENTIALS_FULL_URI",
    ):
        require(forbidden not in environment_names, f"forbidden backend environment name: {forbidden}")
    require("CREDENTIALS_DIRECTORY" in environment_names, "systemd credential directory metadata is absent")

    release_path = str(CURRENT.resolve(strict=True))
    expected_release = f"/opt/craxii/releases/0.0.1-{arguments.deployment_commit[:12]}"
    require(release_path == expected_release, "active release does not match deployment commit")
    release = release_record(pathlib.Path(release_path), arguments.deployment_commit)
    executable = str(pathlib.Path(f"/proc/{main_pid}/exe").resolve(strict=True))
    require(executable == f"{expected_release}/craxii-server", "MainPID executable is not immutable release")

    data_mount = mount_record(DATA_MOUNT)
    data_source = data_mount["source"].split("[", 1)[0]
    data_uuid = run(["/usr/sbin/blkid", "-s", "UUID", "-o", "value", data_source])
    require(data_uuid == arguments.data_uuid, "persistent data filesystem UUID mismatch")
    require(data_mount["fstype"] == "ext4" and data_mount["fsroot"] == "/", "data mount mismatch")
    bind_mounts = {target: mount_record(pathlib.Path(target)) for target in BIND_MOUNTS}
    for target, expected_root in BIND_MOUNTS.items():
        require(bind_mounts[target]["fsroot"] == expected_root, f"bind mount root mismatch: {target}")

    live = health("/health/live")
    ready = health("/health/ready")
    require(live == {"status": 200, "state": "live"}, "liveness check failed")
    require(ready == {"status": 200, "state": "ready"}, "readiness check failed")
    listeners = run(["/usr/bin/ss", "-H", "-ltn", "sport = :8080"])
    listener_addresses = sorted(
        line.split()[3] for line in listeners.splitlines() if len(line.split()) >= 4
    )
    require(listener_addresses == ["127.0.0.1:8080"], "backend listener is not loopback-only")

    database = database_record()
    require(database["quick_check"] == "ok", "SQLite quick_check failed")
    require(database["journal_mode"] == "wal", "SQLite is not in WAL mode")
    require(database["foreign_key_violations"] == 0, "SQLite foreign-key check failed")
    require(database["applied_schema_version"] == 5, "SQLite schema version mismatch")
    require(database["terminal_evidence_violations"] == 0, "ambiguous terminal tool evidence exists")
    require(database["terminal_current_attempt_violations"] == 0, "terminal work retains a current attempt")
    require(database["current_runtime"]["runtime_instance_id"], "runtime identity is absent")
    require(database["current_runtime"]["process_id"] == main_pid, "runtime PID does not match systemd")
    require(database["current_runtime"]["git_revision"] == arguments.deployment_commit, "runtime Git revision mismatch")
    require(database["current_runtime"]["schema_version"] == 5, "runtime schema version mismatch")
    require(database["ambiguity"]["active_work"] == 0, "active canonical work blocks safe restart/reboot")
    recovery = database["current_recovery"]["payload"]
    require(recovery.get("runtime_instance_id") == database["current_runtime"]["runtime_instance_id"], "recovery runtime mismatch")
    require(recovery.get("cleanup_unconfirmed") == 0, "startup recovery has unconfirmed cleanup")

    linux_boot_id = pathlib.Path("/proc/sys/kernel/random/boot_id").read_text(encoding="ascii").strip()
    require(database["current_runtime"]["linux_boot_id"] == linux_boot_id, "runtime Linux boot ID mismatch")

    cgroup = cgroup_record(main_pid, properties["ControlGroup"])
    require(cgroup["leak_status"] == "clean", "execution cgroup residue is present")
    credential = credential_record(main_pid)
    workspace_sentinel = workspace_sentinel_record()
    evidence_sentinel = file_record(EVIDENCE_SENTINEL, include_hash=True)
    require(evidence_sentinel["owner"] == "root", "evidence sentinel owner mismatch")
    final_properties = systemd_properties()
    require(
        final_properties["MainPID"] == properties["MainPID"]
        and final_properties["ExecMainStartTimestampMonotonic"]
        == properties["ExecMainStartTimestampMonotonic"],
        "service changed while production evidence was being captured",
    )

    return {
        "format": "craxii-stage27-production-evidence-v1",
        "phase": arguments.phase,
        "captured_at": dt.datetime.now(dt.timezone.utc).isoformat(timespec="microseconds"),
        "deployment": {
            "git_revision": arguments.deployment_commit,
            "release_path": release_path,
            "current_symlink": str(CURRENT),
            "release": release,
        },
        "host": {
            "linux_boot_id": linux_boot_id,
            "architecture": os.uname().machine,
        },
        "storage": {
            "data_filesystem_uuid": data_uuid,
            "data_mount": data_mount,
            "bind_mounts": bind_mounts,
        },
        "database": database,
        "workspace_sentinel": workspace_sentinel,
        "evidence_sentinel": evidence_sentinel,
        "systemd": properties,
        "process": {
            "main_pid": main_pid,
            "owner": process_owner,
            "group": process_group,
            "executable": executable,
            "cwd": process_cwd,
            "status": {
                field: status_fields[field]
                for field in (
                    "Uid", "Gid", "Groups", "CapInh", "CapPrm", "CapEff", "CapBnd",
                    "CapAmb", "NoNewPrivs", "Umask"
                )
            },
            "environment_names": environment_names,
        },
        "health": {"live": live, "ready": ready, "listeners": listener_addresses},
        "cgroup": cgroup,
        "credential_boundary": credential,
        "validation": {item: "PASS" for item in arguments.validation},
    }


def lookup_path(document: dict[str, Any], dotted: str) -> Any:
    value: Any = document
    for part in dotted.split("."):
        value = value[part]
    return value


def read_evidence_document(path: Any) -> dict[str, Any]:
    if isinstance(path, pathlib.Path):
        record = file_record(path, include_hash=False)
        require(record["owner"] == "root", "evidence input owner mismatch")
        require(record["mode"] == "0600", "evidence input mode mismatch")
    return json.loads(path.read_text(encoding="utf-8"))


def compare(arguments: argparse.Namespace) -> dict[str, Any]:
    before = read_evidence_document(arguments.before)
    after = read_evidence_document(arguments.after)
    require(before["format"] == after["format"] == "craxii-stage27-production-evidence-v1", "evidence format mismatch")
    checks: list[str] = []

    stable_paths = [
        "deployment",
        "storage.data_filesystem_uuid",
        "database.craxii_id",
        "database.applied_schema_version",
        "database.stable_counts",
        "database.states",
        "database.ambiguity",
        "database.artifact_sentinel",
        "workspace_sentinel",
        "evidence_sentinel.sha256",
        "evidence_sentinel.bytes",
        "credential_boundary",
    ]
    for path in stable_paths:
        require(lookup_path(before, path) == lookup_path(after, path), f"restart/reboot changed {path}")
        checks.append(f"stable:{path}")
    for target, expected_root in BIND_MOUNTS.items():
        require(
            after["storage"]["bind_mounts"][target]["fsroot"] == expected_root,
            f"post-transition bind mount mismatch: {target}",
        )
        checks.append(f"bind:{target}")

    before_runtime = before["database"]["current_runtime"]
    after_runtime = after["database"]["current_runtime"]
    require(
        before_runtime["runtime_instance_id"] != after_runtime["runtime_instance_id"],
        "runtime instance ID did not change",
    )
    require(before["process"]["main_pid"] != after["process"]["main_pid"], "service MainPID did not change")
    previous = next(
        (
            row
            for row in after["database"]["runtime_history"]
            if row["runtime_instance_id"] == before_runtime["runtime_instance_id"]
        ),
        None,
    )
    require(previous is not None, "pre-transition runtime is absent from durable history")
    require(previous["state"] == "stopped", "pre-transition runtime did not stop")
    require(previous["stop_reason"] == "graceful_shutdown", "pre-transition runtime was not graceful")
    require(after["health"]["live"] == {"status": 200, "state": "live"}, "post-transition liveness failed")
    require(after["health"]["ready"] == {"status": 200, "state": "ready"}, "post-transition readiness failed")
    require(after["cgroup"]["leak_status"] == "clean", "post-transition execution residue exists")
    require(after["database"]["ambiguity"]["active_work"] == 0, "post-transition active work exists")
    require(
        after["database"]["current_recovery"]["payload"].get("cleanup_unconfirmed") == 0,
        "post-transition recovery cleanup is unconfirmed",
    )
    require(
        after["database"]["max_journal_offset"] > before["database"]["max_journal_offset"],
        "journal head did not advance across the runtime transition",
    )
    require(
        before["database"]["max_journal_offset"]
        < after["database"]["current_recovery"]["journal_offset"]
        <= after["database"]["max_journal_offset"],
        "new runtime recovery event is not ordered after the frozen pre-transition journal head",
    )
    checks.extend(
        [
            "runtime-instance-changed",
            "main-pid-changed",
            "previous-runtime-graceful",
            "live-ready",
            "execution-cgroup-clean",
            "no-active-work",
            "recovery-cleanup-confirmed",
            "journal-head-advanced",
            "recovery-event-after-frozen-head",
        ]
    )

    same_boot = before["host"]["linux_boot_id"] == after["host"]["linux_boot_id"]
    if arguments.mode == "restart":
        require(same_boot, "Linux boot ID changed during service restart")
        checks.append("boot-id-stable")
    else:
        require(not same_boot, "Linux boot ID did not change across claimed reboot")
        checks.append("boot-id-changed")

    return {
        "format": "craxii-stage27-evidence-comparison-v1",
        "mode": arguments.mode,
        "result": "PASS",
        "checks": checks,
        "before_runtime_instance_id": before_runtime["runtime_instance_id"],
        "after_runtime_instance_id": after_runtime["runtime_instance_id"],
        "before_linux_boot_id": before["host"]["linux_boot_id"],
        "after_linux_boot_id": after["host"]["linux_boot_id"],
    }


def write_new_json(path: pathlib.Path, document: dict[str, Any]) -> None:
    path.parent.mkdir(mode=0o700, parents=True, exist_ok=True)
    parent_metadata = path.parent.lstat()
    require(
        stat.S_ISDIR(parent_metadata.st_mode) and not path.parent.is_symlink(),
        f"evidence parent directory is unsafe: {path.parent}",
    )
    require(not path.exists() and not path.is_symlink(), f"refusing to overwrite evidence: {path}")
    descriptor, temporary = tempfile.mkstemp(prefix=f".{path.name}.", dir=path.parent)
    try:
        os.fchmod(descriptor, 0o600)
        with os.fdopen(descriptor, "w", encoding="utf-8") as handle:
            json.dump(document, handle, sort_keys=True, indent=2)
            handle.write("\n")
            handle.flush()
            os.fsync(handle.fileno())
        os.replace(temporary, path)
        directory_descriptor = os.open(path.parent, os.O_RDONLY | os.O_DIRECTORY)
        try:
            os.fsync(directory_descriptor)
        finally:
            os.close(directory_descriptor)
    except BaseException:
        try:
            os.unlink(temporary)
        except FileNotFoundError:
            pass
        raise


def parse_arguments() -> argparse.Namespace:
    parser = argparse.ArgumentParser()
    subcommands = parser.add_subparsers(dest="command", required=True)
    snapshot = subcommands.add_parser("snapshot")
    snapshot.add_argument("--phase", required=True)
    snapshot.add_argument("--deployment-commit", required=True)
    snapshot.add_argument("--data-uuid", required=True)
    snapshot.add_argument("--validation", action="append", default=[])
    snapshot.add_argument("--output", type=pathlib.Path, required=True)
    comparison = subcommands.add_parser("compare")
    comparison.add_argument("--mode", choices=("restart", "reboot"), required=True)
    comparison.add_argument("--before", type=pathlib.Path, required=True)
    comparison.add_argument("--after", type=pathlib.Path, required=True)
    comparison.add_argument("--output", type=pathlib.Path, required=True)
    subcommands.add_parser("validate-workspace-sentinel")
    return parser.parse_args()


def main() -> int:
    arguments = parse_arguments()
    try:
        if arguments.command == "validate-workspace-sentinel":
            workspace_sentinel_record()
        else:
            document = capture(arguments) if arguments.command == "snapshot" else compare(arguments)
            write_new_json(arguments.output, document)
    except (EvidenceError, FileNotFoundError, json.JSONDecodeError, sqlite3.Error, ValueError) as error:
        print(f"error: {error}", file=sys.stderr)
        return 1
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
