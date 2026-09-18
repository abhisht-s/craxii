#!/usr/bin/env python3
"""Deterministic local tests for the CH-6 stopped-service recovery helper."""

from __future__ import annotations

import contextlib
import fcntl
import hashlib
import importlib.util
import io
import json
import os
import pathlib
import shutil
import sqlite3
import stat
import sys
import tempfile
import unittest
from types import ModuleType


ROOT = pathlib.Path(__file__).resolve().parents[2]
HELPER = ROOT / "ops" / "stage27" / "recovery-copy.py"
SYNTHETIC_SECRET = "SYNTHETIC_RECOVERY_CREDENTIAL_CANARY"
SYNTHETIC_ARTIFACT = "SYNTHETIC_RECOVERY_ARTIFACT_CANARY"
SYNTHETIC_MODEL_CONTEXT = "SYNTHETIC_RECOVERY_MODEL_CONTEXT_CANARY"


def load_helper() -> ModuleType:
    spec = importlib.util.spec_from_file_location("stage27_recovery_copy", HELPER)
    assert spec is not None and spec.loader is not None
    module = importlib.util.module_from_spec(spec)
    sys.modules[spec.name] = module
    spec.loader.exec_module(module)
    return module


RECOVERY = load_helper()


def private_directory(path: pathlib.Path) -> None:
    path.mkdir(parents=True)
    os.chmod(path, 0o700)


def create_fixture(root: pathlib.Path, version: int) -> pathlib.Path:
    state = root / "state"
    private_directory(state)
    private_directory(state / "db")
    private_directory(state / "locks")
    database = state / "db" / "craxii.sqlite3"
    lock = state / "locks" / "craxii.lock"
    lock.touch(mode=0o600)
    os.chmod(lock, 0o600)
    connection = sqlite3.connect(database)
    connection.execute("PRAGMA foreign_keys = OFF")
    connection.execute(
        """
        CREATE TABLE _sqlx_migrations (
            version BIGINT PRIMARY KEY,
            description TEXT NOT NULL,
            installed_on TIMESTAMP NOT NULL DEFAULT CURRENT_TIMESTAMP,
            success BOOLEAN NOT NULL,
            checksum BLOB NOT NULL,
            execution_time BIGINT NOT NULL
        )
        """
    )
    contracts = RECOVERY._migration_contracts()
    for path, contract, checksum in contracts[:version]:
        if contract.version == 6:
            connection.execute("CREATE TEMP TABLE ch1_owner_seed (user_id TEXT NOT NULL)")
        connection.executescript(path.read_text(encoding="utf-8"))
        connection.execute(
            "INSERT INTO _sqlx_migrations "
            "(version, description, success, checksum, execution_time) "
            "VALUES (?, ?, 1, ?, 0)",
            (contract.version, contract.description, checksum),
        )
    connection.commit()
    connection.close()
    os.chmod(database, 0o600)
    credentials = state / "synthetic-credentials"
    private_directory(credentials)
    credential = credentials / "provider"
    credential.write_text(SYNTHETIC_SECRET, encoding="utf-8")
    os.chmod(credential, 0o600)
    artifacts = state / "synthetic-artifacts"
    private_directory(artifacts)
    artifact = artifacts / "artifact"
    artifact.write_text(SYNTHETIC_ARTIFACT, encoding="utf-8")
    os.chmod(artifact, 0o600)
    return state


def digest(path: pathlib.Path) -> str:
    return hashlib.sha256(path.read_bytes()).hexdigest()


def inactive() -> object:
    return RECOVERY.ServiceState("inactive", 0)


class RecoveryCopyTests(unittest.TestCase):
    def test_v5_v6_v7_copy_integrity_migrations_manifest_and_restore_validation(self) -> None:
        for version in (5, 6, 7):
            with self.subTest(version=version), tempfile.TemporaryDirectory() as temporary:
                root = pathlib.Path(temporary)
                os.chmod(root, 0o700)
                state = create_fixture(root, version)
                destination_root = root / "recovery"
                private_directory(destination_root)
                database = destination_root / f"v{version}.sqlite3"
                manifest = destination_root / f"v{version}.manifest.json"
                source = state / "db" / "craxii.sqlite3"
                source_before = digest(source)
                credential_before = digest(state / "synthetic-credentials" / "provider")
                artifact_before = digest(state / "synthetic-artifacts" / "artifact")

                report = RECOVERY.create_recovery_copy(
                    state,
                    database,
                    manifest,
                    repository_sha="1" * 40,
                    service_state_reader=inactive,
                )
                self.assertEqual(report.schema_version, version)
                self.assertEqual(len(report.migrations), version)
                self.assertEqual(stat.S_IMODE(database.stat().st_mode), 0o600)
                self.assertEqual(stat.S_IMODE(manifest.stat().st_mode), 0o600)
                self.assertEqual(source_before, digest(source))
                self.assertEqual(
                    credential_before,
                    digest(state / "synthetic-credentials" / "provider"),
                )
                self.assertEqual(
                    artifact_before,
                    digest(state / "synthetic-artifacts" / "artifact"),
                )
                payload = json.loads(manifest.read_text(encoding="utf-8"))
                self.assertEqual(payload["source_schema_version"], version)
                self.assertEqual(payload["schema_ceiling"], 7)
                self.assertEqual(payload["database_sha256"], digest(database))
                self.assertEqual(payload["database_bytes"], database.stat().st_size)
                self.assertEqual(payload["database_mode"], "0600")
                serialized = manifest.read_text(encoding="utf-8")
                for canary in (
                    SYNTHETIC_SECRET,
                    SYNTHETIC_ARTIFACT,
                    SYNTHETIC_MODEL_CONTEXT,
                ):
                    self.assertNotIn(canary, serialized)

                validated = RECOVERY.validate_recovery_copy(database, manifest)
                self.assertEqual(validated, report)
                emitted = io.StringIO()
                errors = io.StringIO()
                with contextlib.redirect_stdout(emitted), contextlib.redirect_stderr(errors):
                    result = RECOVERY.main(
                        [
                            "validate",
                            "--database",
                            str(database),
                            "--manifest",
                            str(manifest),
                        ]
                    )
                self.assertEqual(result, 0)
                self.assertFalse(errors.getvalue())
                self.assertIn("recovery_copy_valid", emitted.getvalue())
                for canary in (
                    SYNTHETIC_SECRET,
                    SYNTHETIC_ARTIFACT,
                    SYNTHETIC_MODEL_CONTEXT,
                ):
                    self.assertNotIn(canary, emitted.getvalue())

                replacement = root / "inactive-replacement"
                private_directory(replacement)
                restored = replacement / "craxii.sqlite3"
                shutil.copyfile(database, restored)
                os.chmod(restored, 0o600)
                restored_report = RECOVERY.validate_recovery_copy(restored, manifest)
                self.assertEqual(restored_report.database_sha256, report.database_sha256)

    def test_backup_api_includes_committed_wal_state_without_mutating_source(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            root = pathlib.Path(temporary)
            os.chmod(root, 0o700)
            state = create_fixture(root, 7)
            source = state / "db" / "craxii.sqlite3"
            writer = sqlite3.connect(source)
            writer.execute("PRAGMA journal_mode = WAL")
            writer.execute("PRAGMA wal_autocheckpoint = 0")
            reader = sqlite3.connect(source)
            reader.execute("BEGIN")
            reader.execute("SELECT COUNT(*) FROM _sqlx_migrations").fetchone()
            writer.execute(
                "UPDATE _sqlx_migrations SET execution_time = 7007 WHERE version = 7"
            )
            writer.commit()
            writer.close()
            wal = pathlib.Path(str(source) + "-wal")
            shm = pathlib.Path(str(source) + "-shm")
            self.assertTrue(wal.exists())
            os.chmod(source, 0o600)
            os.chmod(wal, 0o600)
            os.chmod(shm, 0o600)
            source_before = digest(source)
            wal_before = digest(wal)

            recovery = root / "recovery"
            private_directory(recovery)
            destination = recovery / "wal-consistent.sqlite3"
            manifest = recovery / "wal-consistent.manifest.json"
            report = RECOVERY.create_recovery_copy(
                state,
                destination,
                manifest,
                service_state_reader=inactive,
            )
            self.assertEqual(report.schema_version, 7)
            copied = sqlite3.connect(f"{destination.as_uri()}?mode=ro", uri=True)
            try:
                self.assertEqual(
                    copied.execute(
                        "SELECT execution_time FROM _sqlx_migrations WHERE version = 7"
                    ).fetchone(),
                    (7007,),
                )
            finally:
                copied.close()
            self.assertEqual(source_before, digest(source))
            self.assertEqual(wal_before, digest(wal))
            reader.close()

    def test_active_service_lock_contention_and_overwrite_refuse_before_copy(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            root = pathlib.Path(temporary)
            os.chmod(root, 0o700)
            state = create_fixture(root, 7)
            recovery = root / "recovery"
            private_directory(recovery)

            active_destination = recovery / "active.sqlite3"
            active_manifest = recovery / "active.json"
            with self.assertRaisesRegex(RECOVERY.RecoveryError, "inactive"):
                RECOVERY.create_recovery_copy(
                    state,
                    active_destination,
                    active_manifest,
                    service_state_reader=lambda: RECOVERY.ServiceState("active", 123),
                )
            self.assertFalse(active_destination.exists())
            self.assertFalse(active_manifest.exists())

            descriptor = os.open(state / "locks" / "craxii.lock", os.O_RDWR)
            try:
                fcntl.flock(descriptor, fcntl.LOCK_EX | fcntl.LOCK_NB)
                with self.assertRaisesRegex(RECOVERY.RecoveryError, "already owned"):
                    RECOVERY.create_recovery_copy(
                        state,
                        recovery / "locked.sqlite3",
                        recovery / "locked.json",
                        service_state_reader=inactive,
                    )
            finally:
                os.close(descriptor)

            existing = recovery / "existing.sqlite3"
            existing.write_bytes(b"preserve-existing")
            os.chmod(existing, 0o600)
            before = existing.read_bytes()
            with self.assertRaisesRegex(RECOVERY.RecoveryError, "already exists"):
                RECOVERY.create_recovery_copy(
                    state,
                    existing,
                    recovery / "unused.json",
                    service_state_reader=inactive,
                )
            self.assertEqual(existing.read_bytes(), before)

    def test_foreign_key_checksum_digest_and_permissions_fail_closed(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            root = pathlib.Path(temporary)
            os.chmod(root, 0o700)
            state = create_fixture(root, 6)
            source = state / "db" / "craxii.sqlite3"
            connection = sqlite3.connect(source)
            connection.execute("PRAGMA foreign_keys = OFF")
            connection.execute(
                "INSERT INTO channel_accounts "
                "(channel_account_id, craxii_id, provider_key, external_account_id, "
                "lifecycle_state, created_at, disabled_at) "
                "VALUES (?, ?, 'telegram', 'synthetic', 'active', ?, NULL)",
                (
                    "01890f6c-7b3a-7cc0-98f1-2e6f7a8b9c0d",
                    "01890f6c-7b3a-7cc0-98f1-2e6f7a8b9c0e",
                    "2026-09-18T00:00:00.000000Z",
                ),
            )
            connection.commit()
            connection.close()
            os.chmod(source, 0o600)
            recovery = root / "recovery"
            private_directory(recovery)
            destination = recovery / "foreign-key.sqlite3"
            manifest = recovery / "foreign-key.json"
            with self.assertRaisesRegex(RECOVERY.RecoveryError, "foreign_key_check"):
                RECOVERY.create_recovery_copy(
                    state,
                    destination,
                    manifest,
                    service_state_reader=inactive,
                )
            self.assertFalse(destination.exists())
            self.assertFalse(manifest.exists())

            state = create_fixture(root / "checksum-case", 7)
            source = state / "db" / "craxii.sqlite3"
            connection = sqlite3.connect(source)
            connection.execute(
                "UPDATE _sqlx_migrations SET checksum = zeroblob(48) WHERE version = 1"
            )
            connection.commit()
            connection.close()
            os.chmod(source, 0o600)
            with self.assertRaisesRegex(RECOVERY.RecoveryError, "checksum"):
                RECOVERY.create_recovery_copy(
                    state,
                    recovery / "checksum.sqlite3",
                    recovery / "checksum.json",
                    service_state_reader=inactive,
                )

            valid_state = create_fixture(root / "digest-case", 7)
            valid_database = recovery / "valid.sqlite3"
            valid_manifest = recovery / "valid.json"
            RECOVERY.create_recovery_copy(
                valid_state,
                valid_database,
                valid_manifest,
                service_state_reader=inactive,
            )
            manifest_value = json.loads(valid_manifest.read_text(encoding="utf-8"))
            manifest_value["database_sha256"] = "0" * 64
            valid_manifest.write_text(
                json.dumps(manifest_value, sort_keys=True) + "\n",
                encoding="utf-8",
            )
            os.chmod(valid_manifest, 0o600)
            with self.assertRaisesRegex(RECOVERY.RecoveryError, "digest mismatch"):
                RECOVERY.validate_recovery_copy(valid_database, valid_manifest)
            os.chmod(valid_database, 0o644)
            with self.assertRaisesRegex(RECOVERY.RecoveryError, "mode"):
                RECOVERY.validate_database(valid_database)

    def test_service_state_parser_is_exact(self) -> None:
        self.assertEqual(
            RECOVERY._parse_systemd_properties("ActiveState=inactive\nMainPID=0\n"),
            RECOVERY.ServiceState("inactive", 0),
        )
        for invalid in (
            "ActiveState=inactive\n",
            "ActiveState=inactive\nMainPID=0\nMystery=yes\n",
            "ActiveState=inactive\nActiveState=failed\nMainPID=0\n",
            "ActiveState=inactive\nMainPID=-1\n",
        ):
            with self.assertRaises(RECOVERY.RecoveryError):
                RECOVERY._parse_systemd_properties(invalid)


if __name__ == "__main__":
    unittest.main()
