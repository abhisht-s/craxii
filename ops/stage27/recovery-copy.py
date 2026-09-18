#!/usr/bin/env python3
"""Create and validate stopped-service SQLite recovery copies for Craxii."""

from __future__ import annotations

import argparse
import contextlib
import dataclasses
import datetime
import fcntl
import hashlib
import json
import os
import pathlib
import re
import sqlite3
import stat
import subprocess
import sys
from collections.abc import Callable, Iterator, Sequence
from typing import Any

sys.dont_write_bytecode = True


TOOL_VERSION = 1
SERVICE = "craxii-server.service"
DATABASE_RELATIVE = pathlib.Path("db/craxii.sqlite3")
LOCK_RELATIVE = pathlib.Path("locks/craxii.lock")
MIN_SUPPORTED_SCHEMA_VERSION = 5
SCHEMA_CEILING = 7
ROOT = pathlib.Path(__file__).resolve().parents[2]
MIGRATIONS = ROOT / "backend" / "migrations"
SHA_PATTERN = re.compile(r"[0-9a-f]{40}")
MANIFEST_KEYS = {
    "tool_version",
    "created_at",
    "source_database",
    "recovery_database",
    "source_schema_version",
    "schema_ceiling",
    "migrations",
    "database_sha256",
    "database_bytes",
    "database_mode",
    "repository_sha",
}


class RecoveryError(RuntimeError):
    """A sanitized, fail-closed recovery operation error."""


@dataclasses.dataclass(frozen=True)
class ServiceState:
    active_state: str
    main_pid: int


@dataclasses.dataclass(frozen=True)
class MigrationRecord:
    version: int
    description: str
    checksum_sha384: str


@dataclasses.dataclass(frozen=True)
class ValidationReport:
    schema_version: int
    migrations: tuple[MigrationRecord, ...]
    database_sha256: str
    database_bytes: int


def _require(condition: bool, message: str) -> None:
    if not condition:
        raise RecoveryError(message)


def _path_metadata(path: pathlib.Path) -> os.stat_result:
    try:
        return path.stat(follow_symlinks=False)
    except OSError as error:
        raise RecoveryError("required recovery path is unavailable") from error


def _require_private_directory(path: pathlib.Path) -> None:
    metadata = _path_metadata(path)
    _require(stat.S_ISDIR(metadata.st_mode), "recovery directory is not a directory")
    _require(not stat.S_ISLNK(metadata.st_mode), "recovery directory is a symlink")
    _require(
        stat.S_IMODE(metadata.st_mode) & 0o077 == 0,
        "recovery directory permissions are not private",
    )


def _require_private_regular_file(path: pathlib.Path, mode: int = 0o600) -> None:
    metadata = _path_metadata(path)
    _require(stat.S_ISREG(metadata.st_mode), "recovery file is not regular")
    _require(not stat.S_ISLNK(metadata.st_mode), "recovery file is a symlink")
    _require(metadata.st_nlink == 1, "recovery file has multiple links")
    _require(stat.S_IMODE(metadata.st_mode) == mode, "recovery file mode is unsafe")


def _require_new_path(path: pathlib.Path, label: str) -> None:
    _require(path.is_absolute(), f"{label} path must be absolute")
    _require(not os.path.lexists(path), f"{label} already exists")
    _require_private_directory(path.parent)


def _database_uri(path: pathlib.Path, *, immutable: bool) -> str:
    suffix = "?mode=ro&immutable=1" if immutable else "?mode=ro"
    return path.as_uri() + suffix


def _parse_systemd_properties(output: str) -> ServiceState:
    values: dict[str, str] = {}
    for line in output.splitlines():
        key, separator, value = line.partition("=")
        _require(separator == "=" and key in {"ActiveState", "MainPID"}, "invalid service state")
        _require(key not in values, "duplicate service state")
        values[key] = value
    _require(set(values) == {"ActiveState", "MainPID"}, "incomplete service state")
    _require(values["MainPID"].isascii() and values["MainPID"].isdigit(), "invalid service MainPID")
    return ServiceState(values["ActiveState"], int(values["MainPID"]))


def read_service_state() -> ServiceState:
    try:
        result = subprocess.run(
            [
                "/usr/bin/systemctl",
                "show",
                SERVICE,
                "--property=ActiveState",
                "--property=MainPID",
            ],
            check=False,
            stdin=subprocess.DEVNULL,
            stdout=subprocess.PIPE,
            stderr=subprocess.DEVNULL,
            text=True,
            timeout=10,
        )
    except (OSError, subprocess.SubprocessError) as error:
        raise RecoveryError("service state could not be verified") from error
    _require(result.returncode == 0, "service state could not be verified")
    return _parse_systemd_properties(result.stdout)


def _require_service_inactive(state: ServiceState) -> None:
    _require(
        state.active_state in {"inactive", "failed"} and state.main_pid == 0,
        "service must be inactive with MainPID 0",
    )


@contextlib.contextmanager
def _exclusive_state_lock(lock_path: pathlib.Path) -> Iterator[None]:
    _require_private_regular_file(lock_path)
    flags = os.O_RDWR | os.O_CLOEXEC
    if hasattr(os, "O_NOFOLLOW"):
        flags |= os.O_NOFOLLOW
    try:
        descriptor = os.open(lock_path, flags)
    except OSError as error:
        raise RecoveryError("Craxii offline lock is unavailable") from error
    try:
        opened = os.fstat(descriptor)
        current = _path_metadata(lock_path)
        _require(
            opened.st_dev == current.st_dev and opened.st_ino == current.st_ino,
            "Craxii offline lock changed during open",
        )
        try:
            fcntl.flock(descriptor, fcntl.LOCK_EX | fcntl.LOCK_NB)
        except BlockingIOError as error:
            raise RecoveryError("Craxii offline lock is already owned") from error
        yield
    finally:
        os.close(descriptor)


def _migration_contracts() -> tuple[tuple[pathlib.Path, MigrationRecord, bytes], ...]:
    contracts: list[tuple[pathlib.Path, MigrationRecord, bytes]] = []
    for path in sorted(MIGRATIONS.glob("[0-9][0-9][0-9][0-9]_*.sql")):
        prefix, description = path.stem.split("_", 1)
        sql = path.read_bytes()
        contracts.append(
            (
                path,
                MigrationRecord(
                    version=int(prefix),
                    description=description.replace("_", " "),
                    checksum_sha384=hashlib.sha384(sql).hexdigest(),
                ),
                hashlib.sha384(sql).digest(),
            )
        )
    _require(
        [contract.version for _, contract, _ in contracts]
        == list(range(1, SCHEMA_CEILING + 1)),
        "repository migration inventory is invalid",
    )
    return tuple(contracts)


def _validate_migrations(connection: sqlite3.Connection) -> tuple[int, tuple[MigrationRecord, ...]]:
    user_version = connection.execute("PRAGMA user_version").fetchone()
    _require(user_version == (0,), "SQLite user_version is incompatible")
    shape = connection.execute("PRAGMA table_info('_sqlx_migrations')").fetchall()
    expected_shape = [
        (0, "version", "BIGINT", 0, None, 1),
        (1, "description", "TEXT", 1, None, 0),
        (2, "installed_on", "TIMESTAMP", 1, "CURRENT_TIMESTAMP", 0),
        (3, "success", "BOOLEAN", 1, None, 0),
        (4, "checksum", "BLOB", 1, None, 0),
        (5, "execution_time", "BIGINT", 1, None, 0),
    ]
    _require(shape == expected_shape, "migration history table shape is invalid")
    rows = connection.execute(
        "SELECT version, description, success, checksum, execution_time, "
        "typeof(version), typeof(description), typeof(installed_on), typeof(success), "
        "typeof(checksum), typeof(execution_time) FROM _sqlx_migrations ORDER BY version"
    ).fetchall()
    _require(bool(rows), "migration history is empty")
    schema_version = rows[-1][0]
    _require(
        isinstance(schema_version, int)
        and MIN_SUPPORTED_SCHEMA_VERSION <= schema_version <= SCHEMA_CEILING,
        "schema version is outside the supported recovery range",
    )
    contracts = _migration_contracts()
    _require(len(rows) == schema_version, "migration history is not contiguous")
    records: list[MigrationRecord] = []
    for index, row in enumerate(rows):
        _, contract, checksum = contracts[index]
        version, description, success, stored_checksum, execution_time = row[:5]
        storage_types = row[5:]
        _require(version == index + 1 == contract.version, "migration history is not contiguous")
        _require(description == contract.description, "migration description is inconsistent")
        _require(success == 1, "migration history contains an unsuccessful migration")
        _require(stored_checksum == checksum, "migration checksum is inconsistent")
        _require(
            isinstance(execution_time, int) and execution_time >= 0,
            "migration execution time is invalid",
        )
        _require(
            storage_types == ("integer", "text", "text", "integer", "blob", "integer"),
            "migration history storage types are invalid",
        )
        records.append(contract)

    tables = {
        row[0]
        for row in connection.execute(
            "SELECT name FROM sqlite_schema WHERE type = 'table'"
        ).fetchall()
    }
    _require("craxii_principals" in tables, "canonical schema is incomplete")
    _require(
        ("channel_accounts" in tables) == (schema_version >= 6),
        "channel schema does not match migration history",
    )
    _require(
        ("outbound_deliveries" in tables) == (schema_version >= 7),
        "delivery schema does not match migration history",
    )
    return schema_version, tuple(records)


def _sha256_file(path: pathlib.Path) -> tuple[str, int]:
    digest = hashlib.sha256()
    size = 0
    with path.open("rb") as source:
        while chunk := source.read(1024 * 1024):
            digest.update(chunk)
            size += len(chunk)
    return digest.hexdigest(), size


def validate_database(database: pathlib.Path) -> ValidationReport:
    _require(database.is_absolute(), "database path must be absolute")
    _require_private_regular_file(database)
    for suffix in ("-wal", "-shm"):
        _require(
            not os.path.lexists(pathlib.Path(str(database) + suffix)),
            "recovery database depends on a SQLite sidecar",
        )
    try:
        connection = sqlite3.connect(
            _database_uri(database, immutable=True),
            uri=True,
            timeout=5,
        )
    except sqlite3.Error as error:
        raise RecoveryError("recovery database could not be opened") from error
    try:
        connection.execute("PRAGMA query_only = ON")
        quick = connection.execute("PRAGMA quick_check").fetchall()
        _require(quick == [("ok",)], "SQLite quick_check failed")
        integrity = connection.execute("PRAGMA integrity_check").fetchall()
        _require(integrity == [("ok",)], "SQLite integrity_check failed")
        foreign_keys = connection.execute("PRAGMA foreign_key_check").fetchall()
        _require(not foreign_keys, "SQLite foreign_key_check failed")
        schema_version, migrations = _validate_migrations(connection)
    except sqlite3.Error as error:
        raise RecoveryError("recovery database validation failed") from error
    finally:
        connection.close()
    digest, size = _sha256_file(database)
    return ValidationReport(schema_version, migrations, digest, size)


def _repository_sha(explicit: str | None) -> str | None:
    if explicit is not None:
        _require(bool(SHA_PATTERN.fullmatch(explicit)), "repository SHA is invalid")
        return explicit
    try:
        result = subprocess.run(
            ["git", "rev-parse", "--verify", "HEAD"],
            cwd=ROOT,
            check=False,
            stdin=subprocess.DEVNULL,
            stdout=subprocess.PIPE,
            stderr=subprocess.DEVNULL,
            text=True,
            timeout=10,
        )
    except (OSError, subprocess.SubprocessError):
        return None
    candidate = result.stdout.strip()
    return candidate if result.returncode == 0 and SHA_PATTERN.fullmatch(candidate) else None


def _manifest_payload(
    source: pathlib.Path,
    destination: pathlib.Path,
    report: ValidationReport,
    repository_sha: str | None,
    created_at: str,
) -> dict[str, Any]:
    return {
        "tool_version": TOOL_VERSION,
        "created_at": created_at,
        "source_database": str(source),
        "recovery_database": str(destination),
        "source_schema_version": report.schema_version,
        "schema_ceiling": SCHEMA_CEILING,
        "migrations": [dataclasses.asdict(record) for record in report.migrations],
        "database_sha256": report.database_sha256,
        "database_bytes": report.database_bytes,
        "database_mode": "0600",
        "repository_sha": repository_sha,
    }


def _create_private_file(path: pathlib.Path) -> int:
    flags = os.O_WRONLY | os.O_CREAT | os.O_EXCL | os.O_CLOEXEC
    if hasattr(os, "O_NOFOLLOW"):
        flags |= os.O_NOFOLLOW
    try:
        return os.open(path, flags, 0o600)
    except OSError as error:
        raise RecoveryError("create-once recovery destination could not be created") from error


def _fsync_directory(path: pathlib.Path) -> None:
    flags = os.O_RDONLY | os.O_CLOEXEC
    if hasattr(os, "O_DIRECTORY"):
        flags |= os.O_DIRECTORY
    descriptor = os.open(path, flags)
    try:
        os.fsync(descriptor)
    finally:
        os.close(descriptor)


def _write_manifest(path: pathlib.Path, payload: dict[str, Any]) -> None:
    descriptor = _create_private_file(path)
    try:
        content = (json.dumps(payload, indent=2, sort_keys=True) + "\n").encode("utf-8")
        view = memoryview(content)
        while view:
            written = os.write(descriptor, view)
            _require(written > 0, "recovery manifest write failed")
            view = view[written:]
        os.fsync(descriptor)
    except Exception:
        with contextlib.suppress(OSError):
            path.unlink()
        raise
    finally:
        os.close(descriptor)


def create_recovery_copy(
    state_root: pathlib.Path,
    destination: pathlib.Path,
    manifest: pathlib.Path,
    *,
    repository_sha: str | None = None,
    service_state_reader: Callable[[], ServiceState] = read_service_state,
) -> ValidationReport:
    _require(state_root.is_absolute(), "source state root must be absolute")
    _require_private_directory(state_root)
    source = state_root / DATABASE_RELATIVE
    lock_path = state_root / LOCK_RELATIVE
    _require_private_directory(source.parent)
    _require_private_directory(lock_path.parent)
    _require_private_regular_file(source)
    _require_private_regular_file(lock_path)
    for suffix in ("-wal", "-shm"):
        sidecar = pathlib.Path(str(source) + suffix)
        if os.path.lexists(sidecar):
            _require_private_regular_file(sidecar)
    _require_new_path(destination, "destination database")
    _require_new_path(manifest, "destination manifest")
    _require(
        destination != source and manifest != source and destination != manifest,
        "recovery destination aliases another recovery path",
    )

    _require_service_inactive(service_state_reader())
    destination_created = False
    manifest_created = False
    try:
        with _exclusive_state_lock(lock_path):
            _require_service_inactive(service_state_reader())
            descriptor = _create_private_file(destination)
            os.close(descriptor)
            destination_created = True
            try:
                with (
                    contextlib.closing(
                        sqlite3.connect(
                            _database_uri(source, immutable=False),
                            uri=True,
                            timeout=5,
                        )
                    ) as source_connection,
                    contextlib.closing(
                        sqlite3.connect(str(destination), timeout=5)
                    ) as destination_connection,
                ):
                    source_connection.execute("PRAGMA query_only = ON")
                    source_connection.execute("PRAGMA busy_timeout = 5000")
                    source_connection.backup(destination_connection)
                    destination_connection.execute("PRAGMA journal_mode = DELETE")
                    destination_connection.commit()
            except sqlite3.Error as error:
                raise RecoveryError("SQLite recovery copy failed") from error
            os.chmod(destination, 0o600, follow_symlinks=False)
            with destination.open("rb") as copied:
                os.fsync(copied.fileno())
            _fsync_directory(destination.parent)
            report = validate_database(destination)

        created_at = datetime.datetime.now(datetime.timezone.utc).strftime(
            "%Y-%m-%dT%H:%M:%S.%fZ"
        )
        payload = _manifest_payload(
            source,
            destination,
            report,
            _repository_sha(repository_sha),
            created_at,
        )
        _write_manifest(manifest, payload)
        manifest_created = True
        _fsync_directory(manifest.parent)
        validate_recovery_copy(destination, manifest)
        return report
    except Exception:
        if manifest_created:
            with contextlib.suppress(OSError):
                manifest.unlink()
        if destination_created:
            with contextlib.suppress(OSError):
                destination.unlink()
            for suffix in ("-wal", "-shm", "-journal"):
                with contextlib.suppress(OSError):
                    pathlib.Path(str(destination) + suffix).unlink()
        raise


def _load_manifest(path: pathlib.Path) -> dict[str, Any]:
    _require(path.is_absolute(), "manifest path must be absolute")
    _require_private_regular_file(path)
    try:
        value = json.loads(path.read_text(encoding="utf-8"))
    except (OSError, UnicodeError, json.JSONDecodeError) as error:
        raise RecoveryError("recovery manifest is invalid") from error
    _require(isinstance(value, dict) and set(value) == MANIFEST_KEYS, "recovery manifest shape is invalid")
    return value


def validate_recovery_copy(database: pathlib.Path, manifest: pathlib.Path) -> ValidationReport:
    report = validate_database(database)
    payload = _load_manifest(manifest)
    _require(payload["tool_version"] == TOOL_VERSION, "recovery tool version is incompatible")
    _require(payload["schema_ceiling"] == SCHEMA_CEILING, "recovery schema ceiling is incompatible")
    _require(
        payload["source_schema_version"] == report.schema_version,
        "recovery schema version does not match manifest",
    )
    _require(payload["database_sha256"] == report.database_sha256, "recovery digest mismatch")
    _require(payload["database_bytes"] == report.database_bytes, "recovery size mismatch")
    _require(payload["database_mode"] == "0600", "recovery mode manifest is invalid")
    _require(
        payload["migrations"] == [dataclasses.asdict(record) for record in report.migrations],
        "recovery migration manifest is inconsistent",
    )
    repository_sha = payload["repository_sha"]
    _require(
        repository_sha is None
        or (isinstance(repository_sha, str) and SHA_PATTERN.fullmatch(repository_sha)),
        "recovery repository SHA is invalid",
    )
    created_at = payload["created_at"]
    _require(isinstance(created_at, str) and created_at.endswith("Z"), "recovery timestamp is invalid")
    _require(isinstance(payload["source_database"], str), "source database manifest path is invalid")
    _require(isinstance(payload["recovery_database"], str), "recovery database manifest path is invalid")
    return report


def _absolute_path(value: str) -> pathlib.Path:
    path = pathlib.Path(value)
    if not path.is_absolute():
        raise argparse.ArgumentTypeError("path must be absolute")
    return path


def parse_arguments(arguments: Sequence[str]) -> argparse.Namespace:
    parser = argparse.ArgumentParser(description=__doc__)
    commands = parser.add_subparsers(dest="operation", required=True)
    create = commands.add_parser("create")
    create.add_argument("--source-state-root", required=True, type=_absolute_path)
    create.add_argument("--destination-db", required=True, type=_absolute_path)
    create.add_argument("--manifest", required=True, type=_absolute_path)
    create.add_argument("--repository-sha")
    validate = commands.add_parser("validate")
    validate.add_argument("--database", required=True, type=_absolute_path)
    validate.add_argument("--manifest", required=True, type=_absolute_path)
    return parser.parse_args(arguments)


def main(arguments: Sequence[str] | None = None) -> int:
    try:
        parsed = parse_arguments(sys.argv[1:] if arguments is None else arguments)
        if parsed.operation == "create":
            report = create_recovery_copy(
                parsed.source_state_root,
                parsed.destination_db,
                parsed.manifest,
                repository_sha=parsed.repository_sha,
            )
            print(
                "recovery_copy_created"
                f"\tschema_version={report.schema_version}"
                f"\tdatabase_sha256={report.database_sha256}"
                f"\tdatabase={parsed.destination_db}"
                f"\tmanifest={parsed.manifest}"
            )
        else:
            report = validate_recovery_copy(parsed.database, parsed.manifest)
            print(
                "recovery_copy_valid"
                f"\tschema_version={report.schema_version}"
                f"\tdatabase_sha256={report.database_sha256}"
                f"\tdatabase={parsed.database}"
                f"\tmanifest={parsed.manifest}"
            )
        return 0
    except RecoveryError as error:
        print(f"recovery_error: {error}", file=sys.stderr)
        return 1


if __name__ == "__main__":
    raise SystemExit(main())
