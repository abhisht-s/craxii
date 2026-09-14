#!/usr/bin/env python3
"""Capture and compare redacted Stage 27 production-host evidence."""

from __future__ import annotations

import argparse
import collections
import datetime as dt
import grp
import hashlib
import http.client
import json
import os
import pathlib
import pwd
import re
import sqlite3
import stat
import subprocess
import sys
import tempfile
from typing import Any


DATABASE = pathlib.Path("/var/lib/craxii/db/craxii.sqlite3")
ARTIFACT_ROOT = pathlib.Path("/var/lib/craxii/artifacts")
STATE_ROOT = pathlib.Path("/var/lib/craxii")
STATE_DIRECTORIES = (
    STATE_ROOT,
    STATE_ROOT / "db",
    ARTIFACT_ROOT,
    STATE_ROOT / "locks",
)
WORKSPACE_ROOT = pathlib.Path("/srv/craxii/workspaces/primary")
WORKSTATION_HOME = pathlib.Path("/home/craxii")
WORKSPACE_SENTINEL = pathlib.Path(
    "/srv/craxii/workspaces/primary/.craxii-stage27-persistence-sentinel-v1"
)
EVIDENCE_SENTINEL = pathlib.Path(
    "/srv/craxii-data/stage27-evidence/persistence-sentinel-v1"
)
EVIDENCE_ROOT = EVIDENCE_SENTINEL.parent
WORKSPACE_SENTINEL_CONTENT = b"craxii-stage27-workspace-persistence-sentinel-v1\n"
EVIDENCE_SENTINEL_CONTENT = b"craxii-stage27-evidence-persistence-sentinel-v1\n"
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
MAX_EVIDENCE_BYTES = 4 * 1024 * 1024
JOURNAL_CHECKPOINT_WINDOW = 256
PHASE_VALIDATIONS = {
    "before-service-restart": (),
    "pre-reboot": (
        "A.persistence",
        "B.systemd-cgroup-cleanup",
        "C.startup-recovery",
        "D.outcome-unknown-no-redispatch",
        "E.real-linux-cancellation",
        "F.graceful-service-restart",
        "G.pre-reboot-evidence",
        "H.terminal-outcome-matrix",
        "I.production-composition",
    ),
    "post-reboot": (
        "post-reboot.persistence",
        "post-reboot.systemd-auto-start",
        "post-reboot.recovery-before-ready",
        "post-reboot.credential-boundary",
        "post-reboot.loopback-operation",
    ),
}
MODE_PHASES = {
    "restart": ("before-service-restart", "pre-reboot"),
    "reboot": ("pre-reboot", "post-reboot"),
}
WORKSPACE_SENTINEL_ACCESS_ACL = {
    "user:": "rw-",
    "user:craxii-server": "r-x",
    "group:": "---",
    "mask:": "r--",
    "other:": "---",
}
WORKSPACE_DIRECTORY_ACCESS_ACL = {
    "user:": "rwx",
    "user:craxii-server": "r-x",
    "group:": "---",
    "mask:": "r-x",
    "other:": "---",
}
STABLE_TABLES = (
    "_sqlx_migrations",
    "craxii_principals",
    "workstations",
    "workspaces",
    "conversations",
    "client_devices",
    "client_commands",
    "messages",
    "work_items",
    "work_item_inputs",
    "model_invocations",
    "tool_executions",
    "artifacts",
    "context_manifests",
    "context_manifest_sources",
)
STABLE_TABLE_EXCLUDED_COLUMNS = {
    # Startup refreshes this observation. Every identity, generation, capability, provider, and
    # host-shape field remains part of the fingerprint.
    "workstations": frozenset({"last_seen_at"}),
}
TRANSITION_TABLES = frozenset({"journal_events", "runtime_instances", "stream_heads"})
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


def validation_record(phase: str, supplied: list[str] | tuple[str, ...]) -> dict[str, str]:
    require(phase in PHASE_VALIDATIONS, f"unknown evidence phase: {phase}")
    expected = PHASE_VALIDATIONS[phase]
    require(len(supplied) == len(set(supplied)), "duplicate validation attestation")
    require(set(supplied) == set(expected), f"validation attestations do not match phase: {phase}")
    return {item: "PASS" for item in expected}


def validate_snapshot_arguments(arguments: argparse.Namespace) -> dict[str, str]:
    require(
        re.fullmatch(r"[0-9a-f]{40}", arguments.deployment_commit) is not None,
        "deployment commit must be an exact lowercase 40-character revision",
    )
    require(
        re.fullmatch(
            r"[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}",
            arguments.data_uuid,
        )
        is not None,
        "data filesystem UUID is malformed",
    )
    return validation_record(arguments.phase, arguments.validation)


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


def parse_exact_properties(output: str, names: list[str]) -> dict[str, str]:
    properties: dict[str, str] = {}
    expected = set(names)
    for line in output.splitlines():
        require("=" in line, "systemd property output contains a malformed line")
        name, value = line.split("=", 1)
        require(name in expected, f"unexpected systemd property: {name}")
        require(name not in properties, f"duplicate systemd property: {name}")
        properties[name] = value
    require(set(properties) == expected, "systemd property set is incomplete")
    return properties


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
    return parse_exact_properties(output, names)


def health(path: str) -> dict[str, Any]:
    connection = http.client.HTTPConnection("127.0.0.1", 8080, timeout=5)
    try:
        connection.request("GET", path, headers={"Host": "127.0.0.1:8080"})
        response = connection.getresponse()
        body = response.read(65537)
    finally:
        connection.close()
    require(len(body) <= 65536, f"oversized health response for {path}")
    payload = json.loads(body)
    require(isinstance(payload, dict), f"malformed health response for {path}")
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


def normalized_mount_source(record: dict[str, str]) -> str:
    return record["source"].split("[", 1)[0]


def validate_mount_topology(
    data_mount: dict[str, str], bind_mounts: dict[str, dict[str, str]]
) -> str:
    data_source = normalized_mount_source(data_mount)
    require(data_mount["fstype"] == "ext4" and data_mount["fsroot"] == "/", "data mount mismatch")
    require("rw" in data_mount["options"].split(","), "persistent data filesystem is read-only")
    require(set(bind_mounts) == set(BIND_MOUNTS), "persistent bind mount set mismatch")
    for target, expected_root in BIND_MOUNTS.items():
        record = bind_mounts[target]
        require(
            normalized_mount_source(record) == data_source
            and record["fstype"] == "ext4"
            and record["fsroot"] == expected_root,
            f"bind mount source mismatch: {target}",
        )
        require(
            "rw" in record["options"].split(","),
            f"persistent bind mount is read-only: {target}",
        )
    return data_source


def file_record(path: pathlib.Path, *, include_hash: bool) -> dict[str, Any]:
    flags = os.O_RDONLY | os.O_CLOEXEC | os.O_NOFOLLOW
    descriptor = os.open(path, flags)
    try:
        metadata = os.fstat(descriptor)
        require(stat.S_ISREG(metadata.st_mode), f"expected regular file: {path}")
        record: dict[str, Any] = {
            "path": str(path),
            "owner": pwd.getpwuid(metadata.st_uid).pw_name,
            "group": grp.getgrgid(metadata.st_gid).gr_name,
            "mode": f"{stat.S_IMODE(metadata.st_mode):04o}",
            "hard_links": metadata.st_nlink,
            "bytes": metadata.st_size,
        }
        if include_hash:
            digest = hashlib.sha256()
            while True:
                chunk = os.read(descriptor, 1024 * 1024)
                if not chunk:
                    break
                digest.update(chunk)
            record["sha256"] = digest.hexdigest()
        final_metadata = path.lstat()
        require(
            not stat.S_ISLNK(final_metadata.st_mode)
            and (final_metadata.st_dev, final_metadata.st_ino)
            == (metadata.st_dev, metadata.st_ino),
            f"file path changed during evidence capture: {path}",
        )
        return record
    finally:
        os.close(descriptor)


def release_record(path: pathlib.Path, expected_commit: str) -> dict[str, Any]:
    manifest_path = path / ".craxii-stage27-build-manifest"
    manifest = file_record(manifest_path, include_hash=True)
    require(
        (manifest["owner"], manifest["group"], manifest["mode"], manifest["hard_links"])
        == ("root", "root", "0444", 1),
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
            (record["owner"], record["group"], record["mode"]) == expected_metadata
            and record["hard_links"] == 1,
            f"installed release metadata mismatch: {name}",
        )
        require(record["sha256"] == expected_entries[name], f"installed release digest mismatch: {name}")
        files[name] = record
    return {"manifest": manifest, "files": files}


def directory_record(path: pathlib.Path) -> dict[str, Any]:
    metadata = path.lstat()
    require(stat.S_ISDIR(metadata.st_mode), f"expected directory: {path}")
    require(not stat.S_ISLNK(metadata.st_mode), f"symbolic link refused: {path}")
    return {
        "path": str(path),
        "owner": pwd.getpwuid(metadata.st_uid).pw_name,
        "group": grp.getgrgid(metadata.st_gid).gr_name,
        "mode": f"{stat.S_IMODE(metadata.st_mode):04o}",
    }


def parse_posix_access_acl(value: str) -> dict[str, str]:
    entries: dict[str, str] = {}
    for raw_line in value.splitlines():
        line = raw_line.split("#", 1)[0].strip()
        if not line:
            continue
        fields = line.split(":")
        require(len(fields) == 3, "workspace ACL contains a malformed entry")
        kind, qualifier, permissions = fields
        require(
            kind in {"user", "group", "mask", "other"},
            "workspace ACL contains an unexpected entry kind",
        )
        require(
            len(permissions) == 3
            and permissions[0] in {"r", "-"}
            and permissions[1] in {"w", "-"}
            and permissions[2] in {"x", "-"},
            "workspace ACL contains invalid permissions",
        )
        key = f"{kind}:{qualifier}"
        require(key not in entries, "workspace ACL contains a duplicate entry")
        entries[key] = permissions
    return entries


def parse_workspace_directory_acls(value: str) -> tuple[dict[str, str], dict[str, str]]:
    access_lines: list[str] = []
    default_lines: list[str] = []
    for raw_line in value.splitlines():
        line = raw_line.split("#", 1)[0].strip()
        if not line:
            continue
        if line.startswith("default:"):
            default_lines.append(line.removeprefix("default:"))
        else:
            access_lines.append(line)
    return (
        parse_posix_access_acl("\n".join(access_lines)),
        parse_posix_access_acl("\n".join(default_lines)),
    )


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
    require(
        record["bytes"] == len(WORKSPACE_SENTINEL_CONTENT)
        and record["sha256"] == hashlib.sha256(WORKSPACE_SENTINEL_CONTENT).hexdigest(),
        "workspace sentinel content mismatch",
    )
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
    require(record["hard_links"] == 1, "workspace sentinel has unexpected hard links")
    return record


def evidence_sentinel_record() -> dict[str, Any]:
    record = file_record(EVIDENCE_SENTINEL, include_hash=True)
    require(
        record["bytes"] == len(EVIDENCE_SENTINEL_CONTENT)
        and record["sha256"] == hashlib.sha256(EVIDENCE_SENTINEL_CONTENT).hexdigest(),
        "evidence sentinel content mismatch",
    )
    require(
        (record["owner"], record["group"], record["mode"], record["hard_links"])
        == ("root", "root", "0400", 1),
        "evidence sentinel metadata mismatch",
    )
    return record


def validate_workspace_directory_acl(
    owner: str, group: str, mode: str, value: str
) -> dict[str, Any]:
    require(owner == "craxii", "workspace directory owner mismatch")
    require(group == "craxii", "workspace directory group mismatch")
    access, default = parse_workspace_directory_acls(value)
    require(
        access == WORKSPACE_DIRECTORY_ACCESS_ACL,
        "workspace directory access ACL differs from production policy",
    )
    require(
        default == WORKSPACE_DIRECTORY_ACCESS_ACL,
        "workspace directory default ACL differs from production policy",
    )
    expected_mode = "0" + "".join(
        str(permission_digit(access[key])) for key in ("user:", "mask:", "other:")
    )
    require(mode == expected_mode, "workspace directory mode does not reflect its access ACL")
    require(
        masked_permissions(access["user:craxii-server"], access["mask:"]) == "r-x",
        "workspace directory server access mismatch",
    )
    return {"access": access, "default": default, "mode": mode}


def workspace_directory_record() -> dict[str, Any]:
    record = directory_record(WORKSPACE_ROOT)
    acl = run(
        [
            "/usr/bin/getfacl",
            "--absolute-names",
            "--omit-header",
            str(WORKSPACE_ROOT),
        ]
    )
    record["acl"] = validate_workspace_directory_acl(
        record["owner"], record["group"], record["mode"], acl
    )
    return record


def persistent_paths_record() -> dict[str, Any]:
    records: dict[str, Any] = {}
    for path in STATE_DIRECTORIES:
        record = directory_record(path)
        require(
            (record["owner"], record["group"], record["mode"])
            == ("craxii-server", "craxii-server", "0700"),
            f"persistent state directory metadata mismatch: {path}",
        )
        records[str(path)] = record
    home = directory_record(WORKSTATION_HOME)
    require(
        (home["owner"], home["group"], home["mode"])
        == ("craxii", "craxii", "0700"),
        "workstation home metadata mismatch",
    )
    records[str(WORKSTATION_HOME)] = home
    evidence_root = directory_record(EVIDENCE_ROOT)
    require(
        (evidence_root["owner"], evidence_root["group"], evidence_root["mode"])
        == ("root", "root", "0700"),
        "persistent evidence directory metadata mismatch",
    )
    records[str(EVIDENCE_ROOT)] = evidence_root
    records[str(WORKSPACE_ROOT)] = workspace_directory_record()
    return records


def linux_identity_record() -> dict[str, Any]:
    expected = {
        "craxii-server": ("/var/lib/craxii", "/usr/sbin/nologin"),
        "craxii": ("/home/craxii", "/bin/bash"),
    }
    records: dict[str, Any] = {}
    for name, (home, shell) in expected.items():
        account = pwd.getpwnam(name)
        primary_group = grp.getgrgid(account.pw_gid).gr_name
        require(account.pw_uid != 0 and account.pw_gid != 0, f"Linux identity is privileged: {name}")
        require(primary_group == name, f"Linux primary group mismatch: {name}")
        require(account.pw_dir == home and account.pw_shell == shell, f"Linux account mismatch: {name}")
        require(
            set(os.getgrouplist(name, account.pw_gid)) == {account.pw_gid},
            f"Linux identity has supplementary groups: {name}",
        )
        status = run(["/usr/bin/passwd", "-S", name]).split()
        require(len(status) >= 2 and status[0] == name and status[1] == "L", f"Linux account is not locked: {name}")
        records[name] = {
            "uid": account.pw_uid,
            "gid": account.pw_gid,
            "primary_group": primary_group,
            "home": account.pw_dir,
            "shell": account.pw_shell,
            "locked": True,
            "supplementary_groups": [],
        }
    require(
        records["craxii-server"]["uid"] != records["craxii"]["uid"]
        and records["craxii-server"]["gid"] != records["craxii"]["gid"],
        "backend and workstation Linux identities are not separate",
    )
    authorized_keys = WORKSTATION_HOME / ".ssh" / "authorized_keys"
    require(
        not os.path.lexists(authorized_keys),
        "model-controlled Linux identity has SSH authorized keys",
    )
    return records


def credential_record(main_pid: int) -> dict[str, Any]:
    metadata = CREDENTIAL.lstat()
    directory = CREDENTIAL_DIRECTORY.lstat()
    require(stat.S_ISREG(metadata.st_mode), "provider credential is not a regular file")
    require(not CREDENTIAL.is_symlink(), "provider credential is a symbolic link")
    require(stat.S_ISDIR(directory.st_mode), "provider credential directory is invalid")
    require(not CREDENTIAL_DIRECTORY.is_symlink(), "provider credential directory is a symlink")
    credential_metadata = (
        pwd.getpwuid(metadata.st_uid).pw_name,
        grp.getgrgid(metadata.st_gid).gr_name,
        f"{stat.S_IMODE(metadata.st_mode):04o}",
        metadata.st_nlink,
    )
    directory_metadata = (
        pwd.getpwuid(directory.st_uid).pw_name,
        grp.getgrgid(directory.st_gid).gr_name,
        f"{stat.S_IMODE(directory.st_mode):04o}",
    )
    require(
        credential_metadata == ("craxii-server", "craxii-server", "0600", 1),
        "provider credential metadata mismatch",
    )
    require(
        directory_metadata == ("craxii-server", "craxii-server", "0700"),
        "provider credential directory metadata mismatch",
    )
    require(metadata.st_size > 0, "provider credential is empty")

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
        "owner": credential_metadata[0],
        "group": credential_metadata[1],
        "mode": credential_metadata[2],
        "hard_links": credential_metadata[3],
        "directory_owner": directory_metadata[0],
        "directory_group": directory_metadata[1],
        "directory_mode": directory_metadata[2],
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


def sql_identifier(value: str) -> str:
    require(re.fullmatch(r"[a-z_][a-z0-9_]*", value) is not None, "unsafe SQLite identifier")
    return f'"{value}"'


def update_framed_digest(digest: Any, tag: bytes, value: bytes) -> None:
    digest.update(tag)
    digest.update(len(value).to_bytes(8, byteorder="big", signed=False))
    digest.update(value)


def update_sqlite_value_digest(digest: Any, value: Any, table: str) -> None:
    if value is None:
        update_framed_digest(digest, b"N", b"")
    elif isinstance(value, int):
        update_framed_digest(digest, b"I", str(value).encode("ascii"))
    elif isinstance(value, float):
        update_framed_digest(digest, b"F", value.hex().encode("ascii"))
    elif isinstance(value, str):
        update_framed_digest(digest, b"S", value.encode("utf-8"))
    elif isinstance(value, (bytes, bytearray, memoryview)):
        update_framed_digest(digest, b"B", bytes(value))
    else:
        raise EvidenceError(f"unsupported SQLite value in evidence table: {table}")


def stable_table_fingerprint(
    connection: sqlite3.Connection, table: str, excluded_columns: frozenset[str]
) -> dict[str, Any]:
    table_identifier = sql_identifier(table)
    metadata = connection.execute(f"PRAGMA table_info({table_identifier})").fetchall()
    require(bool(metadata), f"stable table is absent: {table}")
    column_names = [str(row["name"]) for row in metadata]
    require(excluded_columns <= set(column_names), f"stable-table exclusion mismatch: {table}")
    selected = [name for name in column_names if name not in excluded_columns]
    primary_key = [
        str(row["name"])
        for row in sorted(metadata, key=lambda row: int(row["pk"]) or len(metadata) + 1)
        if int(row["pk"]) > 0
    ]
    require(bool(primary_key), f"stable table has no primary key: {table}")
    columns_sql = ", ".join(sql_identifier(name) for name in selected)
    order_sql = ", ".join(sql_identifier(name) for name in primary_key)
    digest = hashlib.sha256()
    update_framed_digest(digest, b"T", table.encode("utf-8"))
    for name in selected:
        update_framed_digest(digest, b"C", name.encode("utf-8"))
    row_count = 0
    for row in connection.execute(
        f"SELECT {columns_sql} FROM {table_identifier} ORDER BY {order_sql}"
    ):
        digest.update(b"R")
        row_count += 1
        for value in row:
            update_sqlite_value_digest(digest, value, table)
    return {"columns": selected, "rows": row_count, "sha256": digest.hexdigest()}


def stable_table_fingerprints(connection: sqlite3.Connection) -> dict[str, dict[str, Any]]:
    return {
        table: stable_table_fingerprint(
            connection, table, STABLE_TABLE_EXCLUDED_COLUMNS.get(table, frozenset())
        )
        for table in STABLE_TABLES
    }


def journal_history_fingerprint(connection: sqlite3.Connection) -> dict[str, Any]:
    metadata = connection.execute('PRAGMA table_info("journal_events")').fetchall()
    require(bool(metadata), "journal table is absent")
    columns = [str(row["name"]) for row in metadata]
    require("journal_offset" in columns, "journal offset column is absent")
    columns_sql = ", ".join(sql_identifier(name) for name in columns)
    digest = hashlib.sha256()
    update_framed_digest(digest, b"T", b"journal_events")
    for name in columns:
        update_framed_digest(digest, b"C", name.encode("utf-8"))
    checkpoints: collections.deque[dict[str, Any]] = collections.deque(
        maxlen=JOURNAL_CHECKPOINT_WINDOW
    )
    rows = 0
    max_offset = 0
    for row in connection.execute(
        f'SELECT {columns_sql} FROM "journal_events" ORDER BY "journal_offset"'
    ):
        digest.update(b"R")
        rows += 1
        for value in row:
            update_sqlite_value_digest(digest, value, "journal_events")
        max_offset = int(row["journal_offset"])
        checkpoints.append(
            {"journal_offset": max_offset, "rows": rows, "sha256": digest.hexdigest()}
        )
    require(rows > 0 and max_offset > 0, "journal history is empty")
    return {
        "columns": columns,
        "rows": rows,
        "max_journal_offset": max_offset,
        "sha256": digest.hexdigest(),
        "checkpoints": list(checkpoints),
    }


def stream_head_violations(connection: sqlite3.Connection) -> int:
    return int(
        connection.execute(
            "SELECT COUNT(*) FROM ("
            " SELECT event_stream.stream_id FROM ("
            "  SELECT stream_id, COUNT(*) AS event_count, MAX(stream_seq) AS last_stream_seq"
            "  FROM journal_events GROUP BY stream_id"
            " ) AS event_stream LEFT JOIN stream_heads AS head USING (stream_id)"
            " WHERE head.stream_id IS NULL"
            "    OR head.last_stream_seq <> event_stream.last_stream_seq"
            "    OR event_stream.event_count <> event_stream.last_stream_seq"
            " UNION ALL"
            " SELECT head.stream_id FROM stream_heads AS head"
            " LEFT JOIN journal_events AS event ON event.stream_id = head.stream_id"
            " WHERE event.stream_id IS NULL"
            ")"
        ).fetchone()[0]
    )


def database_record() -> dict[str, Any]:
    require(DATABASE.is_file() and not DATABASE.is_symlink(), "canonical database is absent or unsafe")
    uri = f"file:{DATABASE}?mode=ro"
    connection = sqlite3.connect(uri, uri=True, timeout=5)
    connection.row_factory = sqlite3.Row
    try:
        connection.execute("PRAGMA query_only = ON")
        connection.execute("BEGIN")
        quick_check = connection.execute("PRAGMA quick_check").fetchone()[0]
        integrity_check = [row[0] for row in connection.execute("PRAGMA integrity_check")]
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
                "SELECT runtime_instance_id, craxii_id, workstation_id, workstation_generation, "
                "linux_boot_id, process_id, binary_version, git_revision, schema_version, state, "
                "started_at, stopped_at, stop_reason "
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
        require(isinstance(recovery, dict), "current runtime recovery payload is malformed")
        counts = {
            table: int(connection.execute(f"SELECT COUNT(*) FROM {table}").fetchone()[0])
            for table in STABLE_TABLES
        }
        fingerprints = stable_table_fingerprints(connection)
        require(
            all(fingerprints[table]["rows"] == counts[table] for table in STABLE_TABLES),
            "stable table count changed within the SQLite snapshot",
        )
        journal_history = journal_history_fingerprint(connection)
        projection_violations = stream_head_violations(connection)
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
        max_journal_offset = journal_history["max_journal_offset"]
    finally:
        if connection.in_transaction:
            connection.rollback()
        connection.close()

    artifact: dict[str, Any] | None = None
    if latest_artifact is not None:
        artifact = dict(latest_artifact)
        artifact_root = ARTIFACT_ROOT.resolve(strict=True)
        artifact_candidate = artifact_root / artifact["storage_key"]
        artifact_path = artifact_candidate.resolve(strict=True)
        require(
            artifact_path.is_relative_to(artifact_root),
            "artifact storage key escapes the canonical artifact root",
        )
        file_metadata = file_record(artifact_candidate, include_hash=True)
        require(
            (
                file_metadata["owner"],
                file_metadata["group"],
                file_metadata["mode"],
                file_metadata["hard_links"],
            )
            == ("craxii-server", "craxii-server", "0600", 1),
            "artifact file metadata mismatch",
        )
        require(file_metadata["sha256"] == artifact["sha256"], "artifact digest mismatch")
        require(file_metadata["bytes"] == artifact["captured_byte_count"], "artifact size mismatch")
        artifact["file"] = file_metadata

    database_file = file_record(DATABASE, include_hash=False)
    require(
        (
            database_file["owner"],
            database_file["group"],
            database_file["mode"],
            database_file["hard_links"],
        )
        == ("craxii-server", "craxii-server", "0600", 1),
        "canonical database metadata mismatch",
    )
    return {
        "path": str(DATABASE),
        "file": database_file,
        "quick_check": quick_check,
        "integrity_check": "ok" if integrity_check == ["ok"] else "failed",
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
        "stable_fingerprints": fingerprints,
        "journal_history": journal_history,
        "stream_head_violations": projection_violations,
        "states": states,
        "ambiguity": ambiguity,
        "terminal_evidence_violations": terminal_evidence_violations,
        "terminal_current_attempt_violations": terminal_current_attempt_violations,
        "max_journal_offset": max_journal_offset,
        "artifact_sentinel": artifact,
    }


def database_head_record() -> dict[str, Any]:
    connection = sqlite3.connect(f"file:{DATABASE}?mode=ro", uri=True, timeout=5)
    try:
        connection.execute("PRAGMA query_only = ON")
        connection.execute("BEGIN")
        running = connection.execute(
            "SELECT runtime_instance_id FROM runtime_instances WHERE state = 'running'"
        ).fetchall()
        require(len(running) == 1, "canonical runtime changed during evidence capture")
        return {
            "runtime_instance_id": running[0][0],
            "max_journal_offset": int(
                connection.execute(
                    "SELECT COALESCE(MAX(journal_offset), 0) FROM journal_events"
                ).fetchone()[0]
            ),
            "active_work": int(
                connection.execute(
                    "SELECT COUNT(*) FROM work_items WHERE state IN "
                    "('queued','running','waiting_on_model','waiting_on_tool','cancel_requested')"
                ).fetchone()[0]
            ),
        }
    finally:
        if connection.in_transaction:
            connection.rollback()
        connection.close()


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
    validations = validate_snapshot_arguments(arguments)
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
    bind_mounts = {target: mount_record(pathlib.Path(target)) for target in BIND_MOUNTS}
    data_source = validate_mount_topology(data_mount, bind_mounts)
    data_uuid = run(["/usr/sbin/blkid", "-s", "UUID", "-o", "value", data_source])
    require(data_uuid == arguments.data_uuid, "persistent data filesystem UUID mismatch")
    persistent_paths = persistent_paths_record()
    linux_identities = linux_identity_record()

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
    require(database["integrity_check"] == "ok", "SQLite integrity_check failed")
    require(database["journal_mode"] == "wal", "SQLite is not in WAL mode")
    require(database["foreign_key_violations"] == 0, "SQLite foreign-key check failed")
    require(database["applied_schema_version"] == 5, "SQLite schema version mismatch")
    require(database["stream_head_violations"] == 0, "journal stream-head projection mismatch")
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
    evidence_sentinel = evidence_sentinel_record()
    final_database_head = database_head_record()
    require(
        final_database_head
        == {
            "runtime_instance_id": database["current_runtime"]["runtime_instance_id"],
            "max_journal_offset": database["max_journal_offset"],
            "active_work": 0,
        },
        "canonical state changed during production evidence capture",
    )
    final_properties = systemd_properties()
    require(
        final_properties == properties,
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
            "persistent_paths": persistent_paths,
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
        "linux_identities": linux_identities,
        "validation": validations,
    }


def lookup_path(document: dict[str, Any], dotted: str) -> Any:
    value: Any = document
    for part in dotted.split("."):
        require(
            isinstance(value, dict) and part in value,
            f"evidence field is absent or malformed: {dotted}",
        )
        value = value[part]
    return value


def read_bounded_evidence_file(path: pathlib.Path) -> str:
    flags = os.O_RDONLY | os.O_CLOEXEC | os.O_NOFOLLOW
    descriptor = os.open(path, flags)
    try:
        metadata = os.fstat(descriptor)
        require(stat.S_ISREG(metadata.st_mode), "evidence input is not a regular file")
        require(metadata.st_uid == 0, "evidence input owner mismatch")
        require(stat.S_IMODE(metadata.st_mode) == 0o600, "evidence input mode mismatch")
        require(metadata.st_nlink == 1, "evidence input has unexpected hard links")
        require(metadata.st_size <= MAX_EVIDENCE_BYTES, "evidence input is oversized")
        chunks: list[bytes] = []
        remaining = MAX_EVIDENCE_BYTES + 1
        while remaining > 0:
            chunk = os.read(descriptor, min(65536, remaining))
            if not chunk:
                break
            chunks.append(chunk)
            remaining -= len(chunk)
        content = b"".join(chunks)
        require(len(content) <= MAX_EVIDENCE_BYTES, "evidence input is oversized")
        final_metadata = path.lstat()
        require(
            not stat.S_ISLNK(final_metadata.st_mode)
            and (final_metadata.st_dev, final_metadata.st_ino)
            == (metadata.st_dev, metadata.st_ino),
            "evidence input path changed while it was read",
        )
        return content.decode("utf-8")
    finally:
        os.close(descriptor)


def read_evidence_document(path: Any) -> dict[str, Any]:
    if isinstance(path, pathlib.Path):
        content = read_bounded_evidence_file(path)
    else:
        content = path.read_text(encoding="utf-8")
        require(isinstance(content, str), "evidence input did not return text")
        require(
            len(content.encode("utf-8")) <= MAX_EVIDENCE_BYTES,
            "evidence input is oversized",
        )
    document = json.loads(content)
    require(isinstance(document, dict), "evidence document must be a JSON object")
    return document


def validate_evidence_envelope(document: dict[str, Any], expected_phase: str) -> None:
    require(
        document.get("format") == "craxii-stage27-production-evidence-v1",
        "evidence format mismatch",
    )
    require(document.get("phase") == expected_phase, "evidence phase mismatch")
    require(
        document.get("validation") == validation_record(expected_phase, list(PHASE_VALIDATIONS[expected_phase])),
        f"evidence validation record mismatch: {expected_phase}",
    )


def compare(arguments: argparse.Namespace) -> dict[str, Any]:
    before = read_evidence_document(arguments.before)
    after = read_evidence_document(arguments.after)
    before_phase, after_phase = MODE_PHASES[arguments.mode]
    validate_evidence_envelope(before, before_phase)
    validate_evidence_envelope(after, after_phase)
    checks: list[str] = []

    stable_paths = [
        "deployment",
        "storage.data_filesystem_uuid",
        "storage.persistent_paths",
        "database.craxii_id",
        "database.applied_schema_version",
        "database.stable_counts",
        "database.stable_fingerprints",
        "database.stream_head_violations",
        "database.states",
        "database.ambiguity",
        "database.artifact_sentinel",
        "workspace_sentinel",
        "evidence_sentinel.sha256",
        "evidence_sentinel.bytes",
        "credential_boundary",
        "linux_identities",
    ]
    for path in stable_paths:
        require(lookup_path(before, path) == lookup_path(after, path), f"restart/reboot changed {path}")
        checks.append(f"stable:{path}")
    require(
        before["database"]["stream_head_violations"]
        == after["database"]["stream_head_violations"]
        == 0,
        "journal stream-head projection is inconsistent",
    )
    for target, expected_root in BIND_MOUNTS.items():
        require(
            after["storage"]["bind_mounts"][target]["fsroot"] == expected_root,
            f"post-transition bind mount mismatch: {target}",
        )
        checks.append(f"bind:{target}")

    before_journal = lookup_path(before, "database.journal_history")
    after_journal = lookup_path(after, "database.journal_history")
    require(
        isinstance(before_journal, dict) and isinstance(after_journal, dict),
        "journal history evidence is malformed",
    )
    require(
        before_journal.get("max_journal_offset") == before["database"]["max_journal_offset"]
        and after_journal.get("max_journal_offset") == after["database"]["max_journal_offset"],
        "journal head evidence is internally inconsistent",
    )
    after_checkpoints = after_journal.get("checkpoints")
    require(isinstance(after_checkpoints, list), "journal checkpoints are malformed")
    matching_checkpoints = [
        checkpoint
        for checkpoint in after_checkpoints
        if isinstance(checkpoint, dict)
        and checkpoint.get("journal_offset") == before_journal.get("max_journal_offset")
    ]
    require(len(matching_checkpoints) == 1, "frozen journal head is outside the checkpoint window")
    prefix = matching_checkpoints[0]
    require(
        prefix.get("rows") == before_journal.get("rows")
        and prefix.get("sha256") == before_journal.get("sha256")
        and after_journal.get("columns") == before_journal.get("columns"),
        "journal history before the transition was changed",
    )
    checks.append("journal-prefix-immutable")

    before_runtime = before["database"]["current_runtime"]
    after_runtime = after["database"]["current_runtime"]
    require(
        before_runtime["runtime_instance_id"] != after_runtime["runtime_instance_id"],
        "runtime instance ID did not change",
    )
    require(
        before_runtime["craxii_id"] == after_runtime["craxii_id"] == before["database"]["craxii_id"],
        "runtime Craxii identity changed",
    )
    require(
        before_runtime["workstation_id"] == after_runtime["workstation_id"]
        and before_runtime["workstation_generation"] == after_runtime["workstation_generation"],
        "runtime workstation identity changed",
    )
    history = after["database"]["runtime_history"]
    require(isinstance(history, list) and len(history) >= 2, "runtime history is incomplete")
    require(
        history[0]["runtime_instance_id"] == after_runtime["runtime_instance_id"],
        "current runtime is not the newest durable runtime",
    )
    previous = history[1]
    require(
        previous["runtime_instance_id"] == before_runtime["runtime_instance_id"],
        "pre-transition runtime is not the immediate durable predecessor",
    )
    for field in (
        "runtime_instance_id",
        "craxii_id",
        "workstation_id",
        "workstation_generation",
        "linux_boot_id",
        "process_id",
        "binary_version",
        "git_revision",
        "schema_version",
        "started_at",
    ):
        require(previous[field] == before_runtime[field], f"pre-transition runtime changed {field}")
    require(previous["state"] == "stopped", "pre-transition runtime did not stop")
    require(previous["stop_reason"] == "graceful_shutdown", "pre-transition runtime was not graceful")
    require(previous["stopped_at"] is not None, "pre-transition runtime stop time is absent")
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
            "runtime-workstation-stable",
            "immediate-runtime-predecessor",
            "runtime-history-bound",
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
        before_start = int(before["systemd"]["ExecMainStartTimestampMonotonic"])
        after_start = int(after["systemd"]["ExecMainStartTimestampMonotonic"])
        require(after_start > before_start, "systemd service incarnation did not advance")
        checks.extend(["boot-id-stable", "systemd-incarnation-advanced"])
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
        try:
            os.link(temporary, path, follow_symlinks=False)
        except FileExistsError as error:
            raise EvidenceError(f"refusing to overwrite evidence: {path}") from error
        os.unlink(temporary)
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
    subcommands.add_parser("validate-sentinels")
    return parser.parse_args()


def main() -> int:
    arguments = parse_arguments()
    try:
        if arguments.command == "validate-workspace-sentinel":
            workspace_sentinel_record()
        elif arguments.command == "validate-sentinels":
            workspace_sentinel_record()
            evidence_sentinel_record()
        else:
            document = capture(arguments) if arguments.command == "snapshot" else compare(arguments)
            write_new_json(arguments.output, document)
    except (
        EvidenceError,
        FileNotFoundError,
        KeyError,
        TypeError,
        AttributeError,
        IndexError,
        OverflowError,
        UnicodeError,
        json.JSONDecodeError,
        sqlite3.Error,
        ValueError,
        OSError,
        subprocess.TimeoutExpired,
        http.client.HTTPException,
    ) as error:
        print(f"error: {error}", file=sys.stderr)
        return 1
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
