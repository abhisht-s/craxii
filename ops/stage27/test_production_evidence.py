#!/usr/bin/env python3
"""Focused tests for the Stage 27 evidence and production-host gate."""

from __future__ import annotations

import argparse
import copy
import hashlib
import importlib.util
import json
import pathlib
import sqlite3
import stat
import subprocess
import tempfile
import unittest
from unittest import mock


HELPER = pathlib.Path(__file__).with_name("production-evidence.py")
VERIFIER = pathlib.Path(__file__).with_name("verify-production-host.sh")
RELEASE_MANIFEST = pathlib.Path(__file__).with_name("verify-release-manifest.sh")
STAGE27_SHELL_SCRIPTS = tuple(sorted(HELPER.parent.glob("*.sh")))
MIGRATIONS = tuple(sorted(HELPER.parents[2].joinpath("backend/migrations").glob("*.sql")))
SPEC = importlib.util.spec_from_file_location("stage27_production_evidence", HELPER)
assert SPEC is not None and SPEC.loader is not None
evidence = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(evidence)

OBSERVED_WORKSPACE_SENTINEL_ACL = """\
user::rw-
user:craxii-server:r-x          #effective:r--
group::---
mask::r--
other::---
"""
OBSERVED_WORKSPACE_DIRECTORY_ACLS = """\
user::rwx
user:craxii-server:r-x
group::---
mask::r-x
other::---
default:user::rwx
default:user:craxii-server:r-x
default:group::---
default:mask::r-x
default:other::---
"""


def shell_function_source(name: str) -> str:
    source = VERIFIER.read_text(encoding="utf-8")
    start = source.index(f"{name}() {{")
    end = source.index("\n}\n", start) + len("\n}\n")
    return source[start:end]


def snapshot(runtime: str, process: int, boot: str, phase: str) -> dict:
    after_transition = runtime == "runtime-after"
    started_at = (
        "2026-09-10T10:01:00.000000Z"
        if after_transition
        else "2026-09-10T10:00:00.000000Z"
    )
    return {
        "format": "craxii-stage27-production-evidence-v1",
        "phase": phase,
        "deployment": {"git_revision": "a" * 40, "release_path": "/release"},
        "host": {"linux_boot_id": boot},
        "storage": {
            "data_filesystem_uuid": "data-uuid",
            "bind_mounts": {
                target: {"fsroot": root} for target, root in evidence.BIND_MOUNTS.items()
            },
            "persistent_paths": {"verified": True},
        },
        "database": {
            "craxii_id": "craxii-id",
            "applied_schema_version": 5,
            "stable_counts": {"work_items": 3},
            "stable_fingerprints": {
                "work_items": {"rows": 3, "sha256": "stable-state"}
            },
            "journal_history": {
                "columns": ["journal_offset", "payload_json"],
                "rows": 13 if after_transition else 10,
                "max_journal_offset": 110 if after_transition else 100,
                "sha256": "after-journal" if after_transition else "frozen-prefix",
                "checkpoints": (
                    [
                        {
                            "journal_offset": 100,
                            "rows": 10,
                            "sha256": "frozen-prefix",
                        },
                        {
                            "journal_offset": 110,
                            "rows": 13,
                            "sha256": "after-journal",
                        },
                    ]
                    if after_transition
                    else [
                        {
                            "journal_offset": 100,
                            "rows": 10,
                            "sha256": "frozen-prefix",
                        }
                    ]
                ),
            },
            "stream_head_violations": 0,
            "states": {"work_items": {"completed": 3}},
            "ambiguity": {
                "active_work": 0,
                "interrupted_work": 1,
                "model_outcome_unknown": 0,
                "tool_outcome_unknown": 1,
            },
            "artifact_sentinel": {"artifact_id": "artifact", "sha256": "artifact-hash"},
            "current_runtime": {
                "runtime_instance_id": runtime,
                "craxii_id": "craxii-id",
                "workstation_id": "workstation-id",
                "workstation_generation": 1,
                "linux_boot_id": boot,
                "process_id": process,
                "binary_version": "0.0.1",
                "git_revision": "a" * 40,
                "schema_version": 5,
                "started_at": started_at,
            },
            "runtime_history": [],
            "current_recovery": {
                "journal_offset": 105 if after_transition else 90,
                "payload": {
                    "runtime_instance_id": runtime,
                    "cleanup_unconfirmed": 0,
                },
            },
            "max_journal_offset": 110 if after_transition else 100,
        },
        "workspace_sentinel": {"sha256": "workspace-hash", "bytes": 10},
        "evidence_sentinel": {"sha256": "evidence-hash", "bytes": 11},
        "credential_boundary": {"present": True, "content_or_hash_inspected": False},
        "linux_identities": {
            "craxii-server": {"uid": 100, "locked": True},
            "craxii": {"uid": 101, "locked": True},
        },
        "process": {"main_pid": process},
        "systemd": {
            "ExecMainStartTimestampMonotonic": "200" if after_transition else "100"
        },
        "health": {
            "live": {"status": 200, "state": "live"},
            "ready": {"status": 200, "state": "ready"},
        },
        "cgroup": {"leak_status": "clean"},
        "validation": evidence.validation_record(
            phase, list(evidence.PHASE_VALIDATIONS[phase])
        ),
    }


def transition(mode: str = "restart") -> tuple[dict, dict, argparse.Namespace]:
    before_phase, after_phase = evidence.MODE_PHASES[mode]
    before = snapshot("runtime-before", 100, "boot-before", before_phase)
    after_boot = "boot-before" if mode == "restart" else "boot-after"
    after = snapshot("runtime-after", 200, after_boot, after_phase)
    after["database"]["runtime_history"] = [
        {
            **after["database"]["current_runtime"],
            "state": "running",
            "stopped_at": None,
            "stop_reason": None,
        },
        {
            **before["database"]["current_runtime"],
            "state": "stopped",
            "stopped_at": "2026-09-10T10:00:59.000000Z",
            "stop_reason": "graceful_shutdown",
        }
    ]
    arguments = argparse.Namespace(mode=mode, before=None, after=None)
    return before, after, arguments


class EvidenceComparisonTests(unittest.TestCase):
    def compare(self, before: dict, after: dict, arguments: argparse.Namespace) -> dict:
        class Document:
            def __init__(self, value: dict) -> None:
                self.value = value

            def read_text(self, *, encoding: str) -> str:
                del encoding
                import json

                return json.dumps(self.value)

        arguments.before = Document(before)
        arguments.after = Document(after)
        return evidence.compare(arguments)

    def test_restart_accepts_stable_state_and_new_graceful_runtime(self) -> None:
        before, after, arguments = transition()
        result = self.compare(before, after, arguments)
        self.assertEqual(result["result"], "PASS")
        self.assertIn("boot-id-stable", result["checks"])

    def test_restart_accepts_main_pid_reuse_when_durable_incarnation_changed(self) -> None:
        before, after, arguments = transition()
        after["process"]["main_pid"] = before["process"]["main_pid"]
        after["database"]["current_runtime"]["process_id"] = before["process"]["main_pid"]
        after["database"]["runtime_history"][0]["process_id"] = before["process"]["main_pid"]
        self.assertEqual(self.compare(before, after, arguments)["result"], "PASS")

    def test_comparison_rejects_canonical_state_change(self) -> None:
        before, after, arguments = transition()
        after = copy.deepcopy(after)
        after["database"]["stable_counts"]["work_items"] += 1
        with self.assertRaisesRegex(evidence.EvidenceError, "stable_counts"):
            self.compare(before, after, arguments)

    def test_comparison_rejects_stable_row_mutation_with_unchanged_counts(self) -> None:
        before, after, arguments = transition()
        after["database"]["stable_fingerprints"]["work_items"]["sha256"] = "mutated"
        with self.assertRaisesRegex(evidence.EvidenceError, "stable_fingerprints"):
            self.compare(before, after, arguments)

    def test_comparison_rejects_changed_or_unprovable_journal_prefix(self) -> None:
        before, after, arguments = transition()
        after["database"]["journal_history"]["checkpoints"][0]["sha256"] = "changed"
        with self.assertRaisesRegex(evidence.EvidenceError, "journal history"):
            self.compare(before, after, arguments)

        before, after, arguments = transition()
        after["database"]["journal_history"]["checkpoints"] = after["database"][
            "journal_history"
        ]["checkpoints"][1:]
        with self.assertRaisesRegex(evidence.EvidenceError, "checkpoint window"):
            self.compare(before, after, arguments)

    def test_comparison_rejects_wrong_phase_or_incomplete_attestations(self) -> None:
        before, after, arguments = transition()
        before["phase"] = "pre-reboot"
        with self.assertRaisesRegex(evidence.EvidenceError, "phase mismatch"):
            self.compare(before, after, arguments)

        before, after, arguments = transition()
        after["validation"].pop("I.production-composition")
        with self.assertRaisesRegex(evidence.EvidenceError, "validation record"):
            self.compare(before, after, arguments)

    def test_comparison_requires_the_immediate_predecessor_runtime(self) -> None:
        before, after, arguments = transition()
        unexpected = copy.deepcopy(after["database"]["runtime_history"][1])
        unexpected["runtime_instance_id"] = "unexpected-runtime"
        after["database"]["runtime_history"].insert(1, unexpected)
        with self.assertRaisesRegex(evidence.EvidenceError, "immediate durable predecessor"):
            self.compare(before, after, arguments)

    def test_comparison_binds_the_predecessor_to_the_frozen_runtime(self) -> None:
        before, after, arguments = transition()
        after["database"]["runtime_history"][1]["git_revision"] = "b" * 40
        with self.assertRaisesRegex(evidence.EvidenceError, "changed git_revision"):
            self.compare(before, after, arguments)

    def test_restart_requires_a_new_systemd_incarnation(self) -> None:
        before, after, arguments = transition()
        after["systemd"]["ExecMainStartTimestampMonotonic"] = "100"
        with self.assertRaisesRegex(evidence.EvidenceError, "incarnation"):
            self.compare(before, after, arguments)

    def test_reboot_requires_a_new_linux_boot_id(self) -> None:
        before, after, arguments = transition("reboot")
        self.assertEqual(self.compare(before, after, arguments)["result"], "PASS")
        after["host"]["linux_boot_id"] = before["host"]["linux_boot_id"]
        with self.assertRaisesRegex(evidence.EvidenceError, "boot ID did not change"):
            self.compare(before, after, arguments)

    def test_comparison_requires_recovery_after_the_frozen_journal_head(self) -> None:
        before, after, arguments = transition()
        after["database"]["current_recovery"]["journal_offset"] = 100
        with self.assertRaisesRegex(evidence.EvidenceError, "recovery event"):
            self.compare(before, after, arguments)

    def test_comparison_requires_journal_head_to_advance(self) -> None:
        before, after, arguments = transition()
        after["database"]["max_journal_offset"] = 100
        with self.assertRaisesRegex(evidence.EvidenceError, "journal head"):
            self.compare(before, after, arguments)

    def test_acl_bearing_workspace_sentinel_uses_effective_mask_permissions(self) -> None:
        result = evidence.validate_workspace_sentinel_acl(
            "craxii", "craxii", "0640", OBSERVED_WORKSPACE_SENTINEL_ACL
        )
        self.assertEqual(result["effective_owner"], "rw-")
        self.assertEqual(result["effective_craxii_server"], "r--")
        self.assertEqual(result["effective_other"], "---")
        self.assertEqual(result["mode"], "0640")

    def test_workspace_sentinel_acl_rejects_unexpected_access(self) -> None:
        variants = [
            OBSERVED_WORKSPACE_SENTINEL_ACL + "user:ssm-user:r--\n",
            OBSERVED_WORKSPACE_SENTINEL_ACL.replace(
                "user:craxii-server:r-x", "user:craxii-server:rwx"
            ),
            OBSERVED_WORKSPACE_SENTINEL_ACL.replace("other::---", "other::r--"),
        ]
        for value in variants:
            with self.subTest(value=value):
                with self.assertRaisesRegex(evidence.EvidenceError, "inherited production policy"):
                    evidence.validate_workspace_sentinel_acl(
                        "craxii", "craxii", "0640", value
                    )

    def test_sentinel_records_require_exact_bytes_not_shell_trimmed_text(self) -> None:
        workspace = {
            "owner": "craxii",
            "group": "craxii",
            "mode": "0640",
            "hard_links": 1,
            "bytes": len(evidence.WORKSPACE_SENTINEL_CONTENT),
            "sha256": hashlib.sha256(evidence.WORKSPACE_SENTINEL_CONTENT).hexdigest(),
        }
        with mock.patch.object(evidence, "file_record", return_value=workspace), mock.patch.object(
            evidence, "run", return_value=OBSERVED_WORKSPACE_SENTINEL_ACL
        ):
            self.assertEqual(evidence.workspace_sentinel_record()["bytes"], workspace["bytes"])
        changed = dict(workspace)
        changed["sha256"] = hashlib.sha256(
            evidence.WORKSPACE_SENTINEL_CONTENT.rstrip(b"\n")
        ).hexdigest()
        with mock.patch.object(evidence, "file_record", return_value=changed):
            with self.assertRaisesRegex(evidence.EvidenceError, "content mismatch"):
                evidence.workspace_sentinel_record()

        durable = {
            "owner": "root",
            "group": "root",
            "mode": "0400",
            "hard_links": 1,
            "bytes": len(evidence.EVIDENCE_SENTINEL_CONTENT),
            "sha256": hashlib.sha256(evidence.EVIDENCE_SENTINEL_CONTENT).hexdigest(),
        }
        with mock.patch.object(evidence, "file_record", return_value=durable):
            self.assertEqual(evidence.evidence_sentinel_record()["bytes"], durable["bytes"])
        durable["bytes"] += 1
        with mock.patch.object(evidence, "file_record", return_value=durable):
            with self.assertRaisesRegex(evidence.EvidenceError, "content mismatch"):
                evidence.evidence_sentinel_record()

    def test_workspace_directory_requires_exact_access_and_inherited_acl(self) -> None:
        result = evidence.validate_workspace_directory_acl(
            "craxii", "craxii", "0750", OBSERVED_WORKSPACE_DIRECTORY_ACLS
        )
        self.assertEqual(result["access"], evidence.WORKSPACE_DIRECTORY_ACCESS_ACL)
        self.assertEqual(result["default"], evidence.WORKSPACE_DIRECTORY_ACCESS_ACL)
        variants = [
            OBSERVED_WORKSPACE_DIRECTORY_ACLS.replace(
                "default:user:craxii-server:r-x", "default:user:craxii-server:rwx"
            ),
            OBSERVED_WORKSPACE_DIRECTORY_ACLS.replace("other::---", "other::r-x", 1),
        ]
        for value in variants:
            with self.subTest(value=value):
                with self.assertRaisesRegex(evidence.EvidenceError, "production policy"):
                    evidence.validate_workspace_directory_acl(
                        "craxii", "craxii", "0750", value
                    )

    def test_systemd_property_parser_rejects_missing_duplicate_and_unknown_fields(self) -> None:
        names = ["ActiveState", "MainPID"]
        self.assertEqual(
            evidence.parse_exact_properties("ActiveState=active\nMainPID=12", names),
            {"ActiveState": "active", "MainPID": "12"},
        )
        for output, message in (
            ("ActiveState=active", "incomplete"),
            ("ActiveState=active\nActiveState=failed\nMainPID=12", "duplicate"),
            ("ActiveState=active\nMainPID=12\nMystery=yes", "unexpected"),
            ("ActiveState=active\nmalformed", "malformed"),
        ):
            with self.subTest(output=output):
                with self.assertRaisesRegex(evidence.EvidenceError, message):
                    evidence.parse_exact_properties(output, names)

    def test_snapshot_arguments_require_exact_phase_attestations_and_identifiers(self) -> None:
        valid = argparse.Namespace(
            deployment_commit="a" * 40,
            data_uuid="12345678-1234-1234-1234-123456789abc",
            phase="pre-reboot",
            validation=list(evidence.PHASE_VALIDATIONS["pre-reboot"]),
        )
        self.assertEqual(
            evidence.validate_snapshot_arguments(valid),
            {item: "PASS" for item in evidence.PHASE_VALIDATIONS["pre-reboot"]},
        )
        variants = [
            {"validation": valid.validation[:-1]},
            {"validation": [*valid.validation, valid.validation[0]]},
            {"phase": "substituted-phase"},
            {"deployment_commit": "A" * 40},
            {"data_uuid": "not-a-filesystem-uuid"},
        ]
        for changes in variants:
            arguments = copy.deepcopy(valid)
            for name, value in changes.items():
                setattr(arguments, name, value)
            with self.subTest(changes=changes):
                with self.assertRaises(evidence.EvidenceError):
                    evidence.validate_snapshot_arguments(arguments)

    def test_mount_topology_requires_writable_binds_from_the_data_filesystem(self) -> None:
        data = {
            "source": "/dev/nvme1n1",
            "fstype": "ext4",
            "fsroot": "/",
            "options": "rw,relatime",
        }
        binds = {
            target: {
                "source": f"/dev/nvme1n1[{root}]",
                "fstype": "ext4",
                "fsroot": root,
                "options": "rw,relatime",
            }
            for target, root in evidence.BIND_MOUNTS.items()
        }
        self.assertEqual(evidence.validate_mount_topology(data, binds), "/dev/nvme1n1")
        for mutation, message in (
            (("/var/lib/craxii", "source", "/dev/root[/state]"), "source mismatch"),
            (("/home/craxii", "options", "ro,relatime"), "read-only"),
        ):
            changed = copy.deepcopy(binds)
            target, field, value = mutation
            changed[target][field] = value
            with self.subTest(mutation=mutation):
                with self.assertRaisesRegex(evidence.EvidenceError, message):
                    evidence.validate_mount_topology(data, changed)

    def test_stable_table_fingerprint_is_ordered_framed_and_excludes_only_refresh_time(self) -> None:
        def connection(rows: list[tuple[str, str, str]]) -> sqlite3.Connection:
            database = sqlite3.connect(":memory:")
            database.row_factory = sqlite3.Row
            database.execute(
                "CREATE TABLE workstations ("
                "workstation_id TEXT PRIMARY KEY, payload TEXT, last_seen_at TEXT) WITHOUT ROWID"
            )
            database.executemany("INSERT INTO workstations VALUES (?, ?, ?)", rows)
            return database

        first = connection([("b", "second", "t1"), ("a", "first", "t1")])
        second = connection([("a", "first", "t2"), ("b", "second", "t2")])
        try:
            first_digest = evidence.stable_table_fingerprint(
                first, "workstations", frozenset({"last_seen_at"})
            )
            second_digest = evidence.stable_table_fingerprint(
                second, "workstations", frozenset({"last_seen_at"})
            )
            self.assertEqual(first_digest, second_digest)
            second.execute("UPDATE workstations SET payload = 'changed' WHERE workstation_id = 'a'")
            changed = evidence.stable_table_fingerprint(
                second, "workstations", frozenset({"last_seen_at"})
            )
            self.assertNotEqual(first_digest["sha256"], changed["sha256"])
        finally:
            first.close()
            second.close()

    def test_journal_commitment_preserves_prefix_and_checks_stream_heads(self) -> None:
        database = sqlite3.connect(":memory:")
        database.row_factory = sqlite3.Row
        try:
            database.executescript(
                "CREATE TABLE journal_events ("
                "journal_offset INTEGER PRIMARY KEY, stream_id TEXT NOT NULL, "
                "stream_seq INTEGER NOT NULL, payload_json TEXT NOT NULL);"
                "CREATE TABLE stream_heads ("
                "stream_id TEXT PRIMARY KEY, last_stream_seq INTEGER NOT NULL);"
                "INSERT INTO journal_events VALUES (1, 'runtime:one', 1, '{}');"
                "INSERT INTO stream_heads VALUES ('runtime:one', 1);"
            )
            before = evidence.journal_history_fingerprint(database)
            self.assertEqual(evidence.stream_head_violations(database), 0)
            database.executescript(
                "INSERT INTO journal_events VALUES (2, 'runtime:one', 2, '{\"next\":true}');"
                "UPDATE stream_heads SET last_stream_seq = 2 WHERE stream_id = 'runtime:one';"
            )
            after = evidence.journal_history_fingerprint(database)
            checkpoint = next(
                item for item in after["checkpoints"] if item["journal_offset"] == 1
            )
            self.assertEqual(checkpoint["sha256"], before["sha256"])
            self.assertEqual(checkpoint["rows"], before["rows"])
            self.assertEqual(evidence.stream_head_violations(database), 0)
            database.execute("UPDATE stream_heads SET last_stream_seq = 3")
            self.assertGreater(evidence.stream_head_violations(database), 0)
        finally:
            database.close()

    def test_stable_fingerprint_inventory_covers_the_current_migrated_schema(self) -> None:
        database = sqlite3.connect(":memory:")
        database.row_factory = sqlite3.Row
        try:
            database.execute(
                "CREATE TABLE _sqlx_migrations ("
                "version INTEGER PRIMARY KEY, description TEXT NOT NULL, "
                "installed_on TEXT NOT NULL, success INTEGER NOT NULL, "
                "checksum BLOB NOT NULL, execution_time INTEGER NOT NULL)"
            )
            for version, migration in enumerate(MIGRATIONS, 1):
                database.executescript(migration.read_text(encoding="utf-8"))
                database.execute(
                    "INSERT INTO _sqlx_migrations VALUES (?, ?, ?, 1, ?, 1)",
                    (
                        version,
                        migration.name,
                        "2026-01-01T00:00:00Z",
                        hashlib.sha256(migration.read_bytes()).digest(),
                    ),
                )
            fingerprints = evidence.stable_table_fingerprints(database)
            self.assertEqual(set(fingerprints), set(evidence.STABLE_TABLES))
            self.assertEqual(fingerprints["_sqlx_migrations"]["rows"], len(MIGRATIONS))
            schema_tables = {
                row[0]
                for row in database.execute(
                    "SELECT name FROM sqlite_schema WHERE type = 'table' "
                    "AND name NOT LIKE 'sqlite_%'"
                )
            }
            self.assertEqual(
                schema_tables,
                set(evidence.STABLE_TABLES) | set(evidence.TRANSITION_TABLES),
            )
        finally:
            database.close()

    def test_oversized_or_non_object_evidence_is_rejected(self) -> None:
        class Document:
            def __init__(self, value: str) -> None:
                self.value = value

            def read_text(self, *, encoding: str) -> str:
                del encoding
                return self.value

        with self.assertRaisesRegex(evidence.EvidenceError, "oversized"):
            evidence.read_evidence_document(
                Document("x" * (evidence.MAX_EVIDENCE_BYTES + 1))
            )
        with self.assertRaisesRegex(evidence.EvidenceError, "JSON object"):
            evidence.read_evidence_document(Document("[]"))

    def test_run_build_places_manifest_after_required_cargo_subcommand(self) -> None:
        source = VERIFIER.read_text(encoding="utf-8")
        start = source.index("run_build() {")
        end = source.index("\n}\n", start)
        run_build = source[start:end]
        self.assertIn('local cargo_subcommand="$1"', run_build)
        command = '"${cargo}" +1.98.0 "${cargo_subcommand}"'
        manifest = '--manifest-path "${checkout}/Cargo.toml" "$@"'
        self.assertIn(command, run_build)
        self.assertIn(manifest, run_build)
        self.assertLess(run_build.index(command), run_build.index(manifest))
        self.assertNotIn(
            '"${cargo}" +1.98.0 --manifest-path',
            run_build,
        )

    def test_independent_check_accumulator_handles_empty_and_named_failure_paths(self) -> None:
        functions = "\n".join(
            shell_function_source(name)
            for name in (
                "run_independent_check",
                "require_last_independent_check_passed",
                "require_independent_checks_passed",
            )
        )
        harness = f"""\
set -euo pipefail
declare -a check_failures=()
last_check_name=""
last_check_status=""
{functions}
quoted_probe() {{
  [[ "$#" -eq 2 && "$1" == "two words" && "$2" == 'literal*value' ]]
}}
run_independent_check empty-array-success quoted_probe "two words" 'literal*value'
require_last_independent_check_passed empty-array-success
require_independent_checks_passed
run_independent_check first-failure /usr/bin/false
run_independent_check safety-pass /usr/bin/true
require_last_independent_check_passed safety-pass
if require_independent_checks_passed; then
  exit 81
fi
[[ "${{#check_failures[@]}}" -eq 1 ]]
[[ "${{check_failures[0]}}" == first-failure:1 ]]
run_independent_check safety-failure /usr/bin/false
if require_last_independent_check_passed safety-failure; then
  exit 82
fi
if run_independent_check '' /usr/bin/true; then
  exit 83
fi
if run_independent_check missing-command; then
  exit 84
fi
"""
        with tempfile.TemporaryDirectory() as directory:
            script = pathlib.Path(directory) / "accumulator-fixture.sh"
            script.write_text(harness, encoding="utf-8")
            completed = subprocess.run(
                ["/bin/bash", str(script)],
                check=False,
                stdout=subprocess.PIPE,
                stderr=subprocess.PIPE,
                text=True,
            )
        self.assertEqual(completed.returncode, 0, completed.stderr)

    def test_success_markers_are_last_and_never_emitted_for_malformed_evidence(self) -> None:
        functions = "\n".join(
            shell_function_source(name) for name in ("print_summary", "emit_pre_reboot_success")
        )
        before, after, _arguments = transition()
        del before
        with tempfile.TemporaryDirectory() as directory:
            root = pathlib.Path(directory)
            valid = root / "valid.json"
            invalid = root / "invalid.json"
            script = root / "marker-fixture.sh"
            valid.write_text(json.dumps(after), encoding="utf-8")
            invalid.write_text("{}", encoding="utf-8")
            script.write_text(
                f"set -euo pipefail\n{functions}\nemit_pre_reboot_success \"$1\"\n",
                encoding="utf-8",
            )
            passed = subprocess.run(
                ["/bin/bash", str(script), str(valid)],
                check=False,
                stdout=subprocess.PIPE,
                stderr=subprocess.PIPE,
                text=True,
            )
            failed = subprocess.run(
                ["/bin/bash", str(script), str(invalid)],
                check=False,
                stdout=subprocess.PIPE,
                stderr=subprocess.PIPE,
                text=True,
            )
        self.assertEqual(passed.returncode, 0, passed.stderr)
        self.assertEqual(
            passed.stdout.splitlines()[-2:],
            ["STAGE27_PRE_REBOOT_GATE=PASS", "HUMAN_EC2_REBOOT_REQUIRED"],
        )
        self.assertNotIn("STAGE27_PRE_REBOOT_GATE=PASS", failed.stdout)
        self.assertNotEqual(failed.returncode, 0)

    def test_verifier_has_no_negative_array_index_or_unchecked_process_substitution(self) -> None:
        source = VERIFIER.read_text(encoding="utf-8")
        self.assertNotRegex(source, r"\[\s*-[0-9]+\s*\]")
        self.assertIn("/bin/sleep infinity &", source)
        self.assertNotRegex(source, r"/bin/sleep\s+[0-9]+\s+&")
        compile_start = source.index("compile_host_tests() {")
        compile_end = source.index("\n}\n", compile_start)
        compile_function = source[compile_start:compile_end]
        self.assertNotIn("< <(", compile_function)
        self.assertIn('>"${binary_list}"', compile_function)

    def test_live_workers_reuse_the_cap_kill_delegated_runner(self) -> None:
        source = VERIFIER.read_text(encoding="utf-8")
        runner = shell_function_source("run_delegated_test")
        live = shell_function_source("run_live_test")
        unit = shell_function_source("run_live_unit_test")
        controller_move = shell_function_source("move_live_test_to_verifier_cgroup")

        self.assertLess(
            runner.index('printf \'0\\n\' >"${cgroup_root}/cgroup.procs"'),
            runner.index("--reuid="),
        )
        self.assertIn(
            "--bounding-set=-all,+kill,+setgid,+setuid,+setpcap", runner
        )
        self.assertIn("--inh-caps=-all,+kill --ambient-caps=-all,+kill", runner)
        self.assertNotIn("sys_admin", source.lower())
        self.assertIn(
            'run_delegated_test "${installed_test}" yes "${test_name}"', live
        )
        self.assertIn(
            'run_delegated_test "${installed_unit_test}" no "${test_name}"', unit
        )
        self.assertIn(
            'printf \'%s\\n\' "${test_pid}" >"${verifier_cgroup_procs}"',
            controller_move,
        )
        restart_start = source.index(
            "run_live_test live_systemd_restart_kills_delegated_execution_but_not_verifier"
        )
        restart_end = source.index('systemctl restart "${service}"', restart_start)
        restart_setup = source[restart_start:restart_end]
        self.assertIn(
            'move_live_test_to_verifier_cgroup "${live_test_pid}"', restart_setup
        )

    def test_stage27_shell_scripts_parse_with_bash(self) -> None:
        for script in STAGE27_SHELL_SCRIPTS:
            with self.subTest(script=script.name):
                parsed = subprocess.run(
                    ["/bin/bash", "-n", str(script)],
                    check=False,
                    stdout=subprocess.PIPE,
                    stderr=subprocess.PIPE,
                    text=True,
                )
                self.assertEqual(parsed.returncode, 0, parsed.stderr)

    def test_create_once_evidence_fsyncs_file_and_parent_directory(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            output = pathlib.Path(directory) / "evidence.json"
            with mock.patch.object(evidence.os, "fsync", wraps=evidence.os.fsync) as fsync:
                evidence.write_new_json(output, {"safe": True})
            self.assertEqual(fsync.call_count, 2)
            self.assertEqual(output.read_text(encoding="utf-8"), '{\n  "safe": true\n}\n')
            self.assertEqual(stat.S_IMODE(output.stat().st_mode), 0o600)
            with self.assertRaisesRegex(evidence.EvidenceError, "refusing to overwrite"):
                evidence.write_new_json(output, {"safe": False})

    def test_create_once_evidence_refuses_an_atomic_name_race(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            output = pathlib.Path(directory) / "evidence.json"
            with mock.patch.object(evidence.os, "link", side_effect=FileExistsError):
                with self.assertRaisesRegex(evidence.EvidenceError, "refusing to overwrite"):
                    evidence.write_new_json(output, {"safe": False})
            self.assertFalse(output.exists())
            self.assertEqual(list(pathlib.Path(directory).iterdir()), [])

    def test_evidence_writer_refuses_a_symlink_parent(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            root = pathlib.Path(directory)
            real = root / "real"
            real.mkdir()
            alias = root / "alias"
            alias.symlink_to(real, target_is_directory=True)
            with self.assertRaisesRegex(evidence.EvidenceError, "parent directory is unsafe"):
                evidence.write_new_json(alias / "evidence.json", {"safe": False})

    def test_release_manifest_binds_every_binary_to_the_exact_commit(self) -> None:
        names = [
            "craxii-server",
            "craxii-admin",
            "craxii-stage27-luna-benchmark",
            "craxii-workstation-launcher",
            "craxii-workstation-reader",
        ]
        commit = "a" * 40
        with tempfile.TemporaryDirectory() as directory:
            root = pathlib.Path(directory)
            entries = []
            for name in names:
                content = f"fixture:{name}".encode()
                (root / name).write_bytes(content)
                entries.append(f"{hashlib.sha256(content).hexdigest()}  {name}")
            (root / ".craxii-stage27-build-manifest").write_text(
                "\n".join([f"commit={commit}", *entries, ""]), encoding="ascii"
            )
            passed = subprocess.run(
                ["/bin/bash", str(RELEASE_MANIFEST), str(root), commit],
                check=False,
                stdout=subprocess.PIPE,
                stderr=subprocess.PIPE,
                text=True,
            )
            self.assertEqual(passed.returncode, 0, passed.stderr)
            (root / names[0]).write_bytes(b"stale target output")
            failed = subprocess.run(
                ["/bin/bash", str(RELEASE_MANIFEST), str(root), commit],
                check=False,
                stdout=subprocess.PIPE,
                stderr=subprocess.PIPE,
                text=True,
            )
            self.assertNotEqual(failed.returncode, 0)


if __name__ == "__main__":
    unittest.main()
