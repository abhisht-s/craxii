#!/usr/bin/env python3
"""Focused tests for the Stage 27 evidence comparison contract."""

from __future__ import annotations

import argparse
import copy
import importlib.util
import pathlib
import unittest


HELPER = pathlib.Path(__file__).with_name("production-evidence.py")
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


def snapshot(runtime: str, process: int, boot: str) -> dict:
    return {
        "format": "craxii-stage27-production-evidence-v1",
        "deployment": {"git_revision": "a" * 40, "release_path": "/release"},
        "host": {"linux_boot_id": boot},
        "storage": {
            "data_filesystem_uuid": "data-uuid",
            "bind_mounts": {
                target: {"fsroot": root} for target, root in evidence.BIND_MOUNTS.items()
            },
        },
        "database": {
            "craxii_id": "craxii-id",
            "applied_schema_version": 4,
            "stable_counts": {"work_items": 3},
            "states": {"work_items": {"completed": 3}},
            "ambiguity": {"active_work": 0, "tool_outcome_unknown": 1},
            "artifact_sentinel": {"artifact_id": "artifact", "sha256": "artifact-hash"},
            "current_runtime": {"runtime_instance_id": runtime},
            "runtime_history": [],
            "current_recovery": {"payload": {"cleanup_unconfirmed": 0}},
        },
        "workspace_sentinel": {"sha256": "workspace-hash", "bytes": 10},
        "evidence_sentinel": {"sha256": "evidence-hash", "bytes": 11},
        "credential_boundary": {"present": True, "content_or_hash_inspected": False},
        "process": {"main_pid": process},
        "health": {
            "live": {"status": 200, "state": "live"},
            "ready": {"status": 200, "state": "ready"},
        },
        "cgroup": {"leak_status": "clean"},
    }


def transition(mode: str = "restart") -> tuple[dict, dict, argparse.Namespace]:
    before = snapshot("runtime-before", 100, "boot-before")
    after_boot = "boot-before" if mode == "restart" else "boot-after"
    after = snapshot("runtime-after", 200, after_boot)
    after["database"]["runtime_history"] = [
        {
            "runtime_instance_id": "runtime-before",
            "state": "stopped",
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

    def test_comparison_rejects_canonical_state_change(self) -> None:
        before, after, arguments = transition()
        after = copy.deepcopy(after)
        after["database"]["stable_counts"]["work_items"] += 1
        with self.assertRaisesRegex(evidence.EvidenceError, "stable_counts"):
            self.compare(before, after, arguments)

    def test_reboot_requires_a_new_linux_boot_id(self) -> None:
        before, after, arguments = transition("reboot")
        self.assertEqual(self.compare(before, after, arguments)["result"], "PASS")
        after["host"]["linux_boot_id"] = before["host"]["linux_boot_id"]
        with self.assertRaisesRegex(evidence.EvidenceError, "boot ID did not change"):
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


if __name__ == "__main__":
    unittest.main()
