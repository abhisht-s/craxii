#!/usr/bin/env python3
"""Deterministic local checks for the CH-6 Stage 27 assets."""

from __future__ import annotations

import contextlib
import importlib.util
import io
import os
import pathlib
import stat
import subprocess
import sys
import tempfile
import tomllib
from types import ModuleType, SimpleNamespace
from typing import Any
from unittest import mock

sys.dont_write_bytecode = True


ROOT = pathlib.Path(__file__).resolve().parents[2]
ASSETS = ROOT / "ops" / "stage27"
TEMPLATE = ASSETS / "config.toml.template"
UNIT = ASSETS / "craxii-server.service"
RENDERER = ASSETS / "render-config.py"
INSTALLER = ASSETS / "install-telegram-credential.py"
UPGRADE = ASSETS / "upgrade-release.sh"
RECOVERY = ASSETS / "recovery-copy.py"
RECOVERY_TESTS = ASSETS / "test_recovery_copy.py"
LOCAL_READINESS = ROOT / "scripts" / "verify-ch6-local"
SYNTHETIC_CHANNEL_ID = "01890f6c-7b3a-7cc0-98f1-2e6f7a8b9c0d"
SYNTHETIC_BOT_ID = 10001
SYNTHETIC_OWNER_ID = 20002


def require(condition: bool, message: str) -> None:
    if not condition:
        raise AssertionError(message)


def load_module(name: str, path: pathlib.Path) -> ModuleType:
    spec = importlib.util.spec_from_file_location(name, path)
    require(spec is not None and spec.loader is not None, f"cannot import {path.name}")
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


def contains_token_key(value: Any) -> bool:
    if isinstance(value, dict):
        return any(
            str(key).lower() == "token"
            or str(key).lower().endswith("_token")
            or contains_token_key(item)
            for key, item in value.items()
        )
    if isinstance(value, list):
        return any(contains_token_key(item) for item in value)
    return False


def enabled_values() -> dict[str, Any]:
    return {
        "enabled": True,
        "channel_account_id": SYNTHETIC_CHANNEL_ID,
        "credential": "telegram_bot",
        "expected_bot_user_id": SYNTHETIC_BOT_ID,
        "owner_telegram_user_id": SYNTHETIC_OWNER_ID,
    }


def verify_configs(renderer: ModuleType) -> tuple[str, str]:
    template_text = TEMPLATE.read_text(encoding="utf-8")
    disabled = tomllib.loads(template_text)
    require(disabled["credentials"] == {
        "source": "systemd",
        "declared": ["openai_provider"],
    }, "disabled credential declaration is not exact")
    require(disabled["telegram"] == {"enabled": False}, "template is not an exact hard-off config")
    require(not contains_token_key(disabled), "template contains a token field")
    require("__REQUIRED_" not in template_text, "template contains an unresolved placeholder")

    enabled_text = renderer.render(template_text, enabled_values())
    enabled = tomllib.loads(enabled_text)
    require(
        enabled["credentials"]["declared"] == ["openai_provider", "telegram_bot"],
        "enabled config does not declare telegram_bot",
    )
    require(enabled["telegram"] == enabled_values(), "enabled Telegram config is not exact")
    require(not contains_token_key(enabled), "rendered enabled config contains a token field")
    require("127.0.0.1:8080" == enabled["server"]["bind_address"], "production bind is not loopback")
    require(
        enabled["server"]["public_base_url"] == "http://127.0.0.1:8080",
        "production base URL is not loopback",
    )

    upgraded_template = template_text.replace('filter = "info"', 'filter = "error"', 1)
    upgraded_text = renderer.render(upgraded_template, enabled_values())
    upgraded = tomllib.loads(upgraded_text)
    require(upgraded["telegram"] == enabled["telegram"], "upgrade changed host Telegram identity")
    require(
        upgraded["credentials"]["declared"] == enabled["credentials"]["declared"],
        "upgrade changed enabled credential declarations",
    )
    require(upgraded["tracing"]["filter"] == "error", "upgrade did not take template-owned changes")
    disabled_upgrade = tomllib.loads(renderer.render(upgraded_template, {"enabled": False}))
    require(
        disabled_upgrade["telegram"] == {"enabled": False}
        and disabled_upgrade["credentials"]["declared"] == ["openai_provider"],
        "upgrade did not preserve disabled state",
    )

    with tempfile.TemporaryDirectory() as temporary_root:
        root = pathlib.Path(temporary_root)
        candidate = root / "candidate.toml"
        installed = root / "installed.toml"
        output = root / "rendered.toml"
        candidate.write_text(upgraded_template, encoding="utf-8")
        installed.write_text(enabled_text, encoding="utf-8")
        result = subprocess.run(
            [
                sys.executable,
                str(RENDERER),
                "--template",
                str(candidate),
                "--preserve-telegram-from",
                str(installed),
                "--output",
                str(output),
            ],
            check=False,
            stdin=subprocess.DEVNULL,
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
            text=True,
        )
        require(result.returncode == 0, "renderer CLI failed the simulated upgrade")
        require(not result.stdout and not result.stderr, "renderer emitted unexpected operator output")
        cli_upgrade = tomllib.loads(output.read_text(encoding="utf-8"))
        require(cli_upgrade["telegram"] == enabled_values(), "renderer CLI changed host values")
        require(cli_upgrade["tracing"]["filter"] == "error", "renderer CLI lost template changes")
        checked = subprocess.run(
            [
                sys.executable,
                str(RENDERER),
                "--template",
                str(candidate),
                "--preserve-telegram-from",
                str(output),
                "--check",
                str(output),
            ],
            check=False,
            stdin=subprocess.DEVNULL,
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
            text=True,
        )
        require(checked.returncode == 0, "rendered deployed-config comparison failed")

        legacy = root / "legacy.toml"
        legacy_output = root / "legacy-upgrade.toml"
        legacy.write_text(
            enabled_text.replace(
                'declared = ["openai_provider", "telegram_bot"]',
                'declared = ["openai_provider"]',
                1,
            ).replace(
                '[telegram]\nenabled = true\n'
                f'channel_account_id = "{SYNTHETIC_CHANNEL_ID}"\n'
                'credential = "telegram_bot"\n'
                f'expected_bot_user_id = {SYNTHETIC_BOT_ID}\n'
                f'owner_telegram_user_id = {SYNTHETIC_OWNER_ID}\n\n',
                "",
                1,
            ),
            encoding="utf-8",
        )
        legacy_result = subprocess.run(
            [
                sys.executable,
                str(RENDERER),
                "--template",
                str(candidate),
                "--preserve-telegram-from",
                str(legacy),
                "--output",
                str(legacy_output),
            ],
            check=False,
            stdin=subprocess.DEVNULL,
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
            text=True,
        )
        require(legacy_result.returncode == 0, "legacy disabled upgrade render failed")
        legacy_upgrade = tomllib.loads(legacy_output.read_text(encoding="utf-8"))
        require(
            legacy_upgrade["telegram"] == {"enabled": False}
            and legacy_upgrade["credentials"]["declared"] == ["openai_provider"],
            "legacy config without Telegram did not upgrade as disabled",
        )

    try:
        renderer.render(
            template_text.replace('filter = "info"', 'filter = "__REQUIRED_FILTER"', 1),
            {"enabled": False},
        )
    except renderer.RenderError:
        pass
    else:
        raise AssertionError("renderer accepted an unresolved placeholder")
    return template_text, enabled_text


def verify_systemd(template_text: str) -> None:
    unit = UNIT.read_text(encoding="utf-8")
    mappings = [
        line
        for line in unit.splitlines()
        if line.startswith("LoadCredential=")
    ]
    require(
        mappings
        == [
            "LoadCredential=openai_provider:/etc/craxii/credentials/openai_provider",
            "LoadCredential=telegram_bot:/etc/craxii/credentials/telegram_bot",
        ],
        "systemd credential mappings are not exact and separate",
    )
    require(mappings[0] != mappings[1], "OpenAI and Telegram credential mappings collide")
    require(
        not any(line.startswith(("Environment=", "EnvironmentFile=")) for line in unit.splitlines()),
        "systemd unit exposes a global environment source",
    )
    exec_start = next(line for line in unit.splitlines() if line.startswith("ExecStart="))
    require("telegram" not in exec_start.lower() and "token" not in exec_start.lower(), "token is exposed in argv")
    require("TELEGRAM_BOT_TOKEN" not in unit, "Telegram token environment name appears in the unit")
    require("TELEGRAM_BOT_TOKEN" not in template_text, "production TOML contains a Telegram token")
    require("webhook" not in unit.lower(), "polling service unexpectedly configures a webhook")
    require(
        not any("caddy" in path.name.lower() for path in ASSETS.iterdir()),
        "Stage 27 unexpectedly contains a Caddy route asset",
    )


def verify_deployment_scripts() -> None:
    bootstrap = (ASSETS / "bootstrap-security-boundary.sh").read_text(encoding="utf-8")
    upgrade = (ASSETS / "upgrade-release.sh").read_text(encoding="utf-8")
    precredential = (ASSETS / "verify-precredential-host.sh").read_text(encoding="utf-8")
    production = (ASSETS / "verify-production-host.sh").read_text(encoding="utf-8")
    linux_boundary = (
        ROOT / "backend" / "src" / "adapters" / "local_workstation.rs"
    ).read_text(encoding="utf-8")
    require("credentials/telegram_bot" in bootstrap, "bootstrap does not guard Telegram precredential state")
    require("config validate" in bootstrap and "render-config.py" in bootstrap, "bootstrap skips rendered Rust validation")
    require(
        "--preserve-telegram-from" in upgrade and "config validate" in upgrade,
        "upgrade does not preserve and Rust-validate Telegram config",
    )
    require(
        "credentials/telegram_bot" in precredential,
        "precredential verification omits the Telegram path",
    )
    require(
        "--preserve-telegram-from /etc/craxii/config.toml" in production,
        "production verification still assumes byte-identical host config",
    )
    require(
        'test -z \\"${{TELEGRAM_BOT_TOKEN-}}\\"' in linux_boundary,
        "Linux workstation boundary does not explicitly reject Telegram token inheritance",
    )


def filesystem_snapshot(root: pathlib.Path) -> tuple[tuple[Any, ...], ...]:
    entries: list[tuple[Any, ...]] = []
    for path in [root, *sorted(root.rglob("*"))]:
        metadata = path.lstat()
        relative = "." if path == root else str(path.relative_to(root))
        payload: bytes | str | None = None
        if stat.S_ISREG(metadata.st_mode):
            payload = path.read_bytes()
        elif stat.S_ISLNK(metadata.st_mode):
            payload = os.readlink(path)
        entries.append(
            (
                relative,
                metadata.st_mode,
                metadata.st_uid,
                metadata.st_gid,
                metadata.st_nlink,
                metadata.st_size,
                payload,
            )
        )
    return tuple(entries)


def require_preflight_failure(
    installer: ModuleType,
    directory: pathlib.Path,
    uid: int,
    gid: int,
) -> str:
    try:
        installer.verify_credential(directory, uid, gid)
    except installer.CredentialError as error:
        return str(error)
    raise AssertionError("unsafe Telegram credential passed upgrade preflight")


def run_verify_cli(
    installer: ModuleType,
    directory: pathlib.Path,
    uid: int,
    gid: int,
) -> tuple[int, str]:
    emitted = io.StringIO()
    identity = SimpleNamespace(pw_uid=uid, pw_gid=gid)
    with (
        contextlib.redirect_stdout(emitted),
        contextlib.redirect_stderr(emitted),
        mock.patch.object(installer, "CREDENTIAL_DIRECTORY", directory),
        mock.patch.object(installer.os, "geteuid", return_value=0),
        mock.patch.object(installer.pwd, "getpwnam", return_value=identity),
        mock.patch.object(
            installer,
            "_service_is_stopped",
            side_effect=AssertionError("verification queried service state"),
        ),
        mock.patch.object(
            installer.getpass,
            "getpass",
            side_effect=AssertionError("verification prompted for credential contents"),
        ),
        mock.patch.object(sys, "argv", [str(INSTALLER), "--verify"]),
    ):
        result = installer.main()
    return result, emitted.getvalue()


def verify_upgrade_credential_preflight(installer: ModuleType, renderer: ModuleType) -> str:
    upgrade = UPGRADE.read_text(encoding="utf-8")
    preflight = (
        'if ! /usr/bin/python3 "${asset_directory}/install-telegram-credential.py" '
        "\\\n  --verify >/dev/null 2>&1; then"
    )
    preflight_position = upgrade.find(preflight)
    trap_position = upgrade.find("trap cleanup EXIT")
    first_staging_mutation = upgrade.find(
        'install -d -o root -g root -m 0700 "${staging_directory}"'
    )
    first_release_mutation = upgrade.find(
        'install -d -o root -g root -m 0755 "${release_directory}"'
    )
    stop_position = upgrade.find('systemctl stop "${service}"', preflight_position)
    pointer_position = upgrade.find('mv -Tf "${temporary_link}" "${current}"')
    start_position = upgrade.find('systemctl start "${service}"')
    require(
        -1 not in {
            preflight_position,
            trap_position,
            first_staging_mutation,
            first_release_mutation,
            stop_position,
            pointer_position,
            start_position,
        },
        "upgrade credential preflight or a protected mutation marker is absent",
    )
    require(
        preflight_position
        < trap_position
        < first_staging_mutation
        < first_release_mutation
        < stop_position
        < pointer_position
        < start_position,
        "upgrade credential preflight does not precede every protected mutation",
    )
    operator_error = (
        "required Telegram credential /etc/craxii/credentials/telegram_bot is missing or unsafe; "
        "install it first with install-telegram-credential.py --install"
    )
    require(operator_error in upgrade, "upgrade lacks the safe credential operator error")

    uid = os.getuid()
    gid = os.getgid()
    telegram_canary = "CXR_FAKE_" + "UPGRADE_TELEGRAM_CANARY_" + "LOCAL_ONLY"
    openai_canary = b"synthetic-openai-fixture"

    with tempfile.TemporaryDirectory() as temporary_root:
        root = pathlib.Path(temporary_root)
        credentials = root / "credentials"
        credentials.mkdir(mode=0o700)
        os.chmod(credentials, 0o700)
        openai = credentials / "openai_provider"
        openai.write_bytes(openai_canary)
        os.chmod(openai, 0o600)

        incumbent = root / "releases" / "legacy"
        incumbent.mkdir(parents=True)
        current = root / "current"
        current.symlink_to(incumbent, target_is_directory=True)
        installed_unit = root / "craxii-server.service"
        installed_unit.write_text("synthetic legacy unit\n", encoding="utf-8")
        installed_config = root / "config.toml"
        installed_config.write_text(
            '[credentials]\nsource = "systemd"\ndeclared = ["openai_provider"]\n\n'
            "[telegram]\nenabled = false\n",
            encoding="utf-8",
        )
        database = root / "state.db"
        database.write_bytes(b"synthetic-state-before-upgrade")
        actions: list[str] = []
        before = filesystem_snapshot(root)

        error = require_preflight_failure(installer, credentials, uid, gid)
        require("credential" in error.lower(), "missing-credential failure is not actionable")
        cli_result, cli_output = run_verify_cli(installer, credentials, uid, gid)
        require(cli_result != 0, "missing credential passed the verifier CLI")
        require(telegram_canary not in cli_output, "verifier CLI exposed token material")
        require(actions == [], "missing credential invoked a stop or candidate-start action")
        require(filesystem_snapshot(root) == before, "missing credential mutated incumbent state")
        require(current.resolve() == incumbent.resolve(), "missing credential changed release pointer")
        require(not (root / "candidate").exists(), "missing credential created candidate state")
        require(
            telegram_canary not in error and telegram_canary not in operator_error,
            "missing-credential error exposed token material",
        )

        unsafe_target = credentials / "telegram_bot"
        source = credentials / "synthetic-source"
        source.write_text(telegram_canary, encoding="utf-8")
        os.chmod(source, 0o600)
        unsafe_target.symlink_to(source.name)
        error = require_preflight_failure(installer, credentials, uid, gid)
        require(telegram_canary not in error, "symlink failure exposed token material")
        unsafe_target.unlink()

        unsafe_target.write_text(telegram_canary, encoding="utf-8")
        os.chmod(unsafe_target, 0o644)
        error = require_preflight_failure(installer, credentials, uid, gid)
        require(telegram_canary not in error, "mode failure exposed token material")
        unsafe_target.unlink()

        unsafe_target.mkdir(mode=0o600)
        error = require_preflight_failure(installer, credentials, uid, gid)
        require(telegram_canary not in error, "non-regular failure exposed token material")
        unsafe_target.rmdir()

        unsafe_target.write_text(telegram_canary, encoding="utf-8")
        os.chmod(unsafe_target, 0o600)
        error = require_preflight_failure(installer, credentials, uid + 1, gid)
        require(telegram_canary not in error, "ownership failure exposed token material")
        unsafe_target.unlink()
        require(actions == [], "invalid credential metadata invoked a service action")

        installer.write_credential(
            credentials,
            telegram_canary.encode(),
            uid,
            gid,
            rotate=False,
        )
        emitted = io.StringIO()
        with (
            contextlib.redirect_stdout(emitted),
            contextlib.redirect_stderr(emitted),
            mock.patch("builtins.open", side_effect=AssertionError("credential content opened")),
            mock.patch.object(
                pathlib.Path,
                "open",
                side_effect=AssertionError("credential content opened"),
            ),
            mock.patch("os.open", side_effect=AssertionError("credential content opened")),
        ):
            installer.verify_credential(credentials, uid, gid)
        require(not emitted.getvalue(), "metadata preflight emitted unexpected output")
        cli_result, cli_output = run_verify_cli(installer, credentials, uid, gid)
        require(cli_result == 0, "installer-produced credential failed the verifier CLI")
        require(telegram_canary not in cli_output, "successful verifier CLI exposed the token")
        actions.append("continue-upgrade")
        require(actions == ["continue-upgrade"], "valid credential did not continue upgrade")

        legacy = tomllib.loads(installed_config.read_text(encoding="utf-8"))
        rendered = tomllib.loads(
            renderer.render(TEMPLATE.read_text(encoding="utf-8"), legacy["telegram"])
        )
        require(
            rendered["telegram"] == {"enabled": False}
            and rendered["credentials"]["declared"] == ["openai_provider"],
            "valid preflight changed the preserved Telegram-disabled config",
        )

    require(
        telegram_canary not in upgrade,
        "synthetic Telegram token appeared in upgrade arguments or source",
    )
    return telegram_canary


def verify_installer(installer: ModuleType) -> str:
    require(
        installer.CREDENTIAL_DIRECTORY == pathlib.Path("/etc/craxii/credentials")
        and installer.CREDENTIAL_NAME == "telegram_bot",
        "Telegram installer destination is not exact",
    )
    canary = "CXR_FAKE_" + "TELEGRAM_SECRET_CANARY_" + "LOCAL_ONLY"
    emitted = io.StringIO()
    with tempfile.TemporaryDirectory() as temporary_root, contextlib.redirect_stdout(emitted), contextlib.redirect_stderr(emitted):
        directory = pathlib.Path(temporary_root) / "credentials"
        directory.mkdir(mode=0o700)
        os.chmod(directory, 0o700)
        uid = os.getuid()
        gid = os.getgid()
        target = directory / "telegram_bot"

        installer.write_credential(directory, canary.encode(), uid, gid, rotate=False)
        installer.verify_credential(directory, uid, gid)
        require(stat.S_IMODE(directory.stat().st_mode) == 0o700, "installer changed directory mode")
        require(target.read_text(encoding="utf-8") == canary, "create-once install changed the secret")
        require(stat.S_IMODE(target.stat().st_mode) == 0o600, "installed credential is not mode 0600")
        try:
            installer.write_credential(directory, b"replacement-refused", uid, gid, rotate=False)
        except installer.CredentialError:
            pass
        else:
            raise AssertionError("create-once install overwrote an existing credential")
        require(target.read_text(encoding="utf-8") == canary, "failed create changed prior credential")

        installer.write_credential(directory, b"synthetic-rotated-value", uid, gid, rotate=True)
        require(
            target.read_bytes() == b"synthetic-rotated-value",
            "rotation did not atomically replace the credential",
        )

        def fail_replace(_source: os.PathLike[str], _target: os.PathLike[str]) -> None:
            raise OSError("synthetic replacement failure")

        try:
            installer.write_credential(
                directory,
                b"synthetic-failed-rotation",
                uid,
                gid,
                rotate=True,
                replace=fail_replace,
            )
        except OSError:
            pass
        else:
            raise AssertionError("synthetic rotation failure unexpectedly succeeded")
        require(
            target.read_bytes() == b"synthetic-rotated-value",
            "failed rotation did not preserve the previous credential",
        )
        require(
            not list(directory.glob(".telegram_bot.tmp.*")),
            "credential operation left a temporary file",
        )

    require(canary not in emitted.getvalue(), "installer/rotator logs exposed the synthetic token")
    source = INSTALLER.read_text(encoding="utf-8")
    require("getpass.getpass" in source, "installer does not use hidden terminal input")
    require("os.replace" in source and "os.fsync" in source, "rotation is not atomic and durable")
    require("sys.argv" not in source, "installer can read a token from process arguments")
    return canary


def verify_recovery_assets() -> None:
    require(RECOVERY.is_file(), "recovery helper is missing")
    require(RECOVERY_TESTS.is_file(), "recovery helper tests are missing")
    require(
        stat.S_IMODE(RECOVERY.stat().st_mode) == 0o755,
        "recovery helper is not executable mode 0755",
    )
    helper = RECOVERY.read_text(encoding="utf-8")
    readme = (ASSETS / "README.md").read_text(encoding="utf-8")
    for contract in (
        "source_connection.backup(destination_connection)",
        "PRAGMA quick_check",
        "PRAGMA integrity_check",
        "PRAGMA foreign_key_check",
        "fcntl.LOCK_EX | fcntl.LOCK_NB",
        '"--property=MainPID"',
        "os.O_EXCL",
        'create.add_argument("--source-state-root"',
        'create.add_argument("--destination-db"',
        'create.add_argument("--manifest"',
        'validate.add_argument("--database"',
    ):
        require(contract in helper, f"recovery helper omits contract: {contract}")
    require("shutil.copy" not in helper, "recovery helper raw-copies the SQLite database")
    require(
        "/etc/craxii/credentials/openai_provider" not in helper
        and "/etc/craxii/credentials/telegram_bot" not in helper,
        "recovery helper references a credential source",
    )
    for procedure in (
        "systemctl start craxii-server.service",
        "systemctl stop craxii-server.service",
        "systemctl restart craxii-server.service",
        "delivery inspect",
        "channel-account disable",
        "recovery-copy.py create",
        "recovery-copy.py validate",
        "V5 candidate startup applies 0006 and 0007",
        "V6 applies 0007",
        "V7 applies none",
        "Inactive-replacement restore boundary",
        "Forward-only migration failure",
        "Do not start the older binary",
    ):
        require(procedure in readme, f"README omits procedure: {procedure}")
    require(
        "/var/lib/craxii-replacement/db/craxii.sqlite3" in readme,
        "restore procedure does not target a new inactive replacement",
    )
    for line in readme.splitlines():
        command = line.strip()
        require(
            not (
                command.startswith(("cp ", "sudo cp "))
                and "/var/lib/craxii/db/craxii.sqlite3" in command
            ),
            "README presents raw copy of the active SQLite main database",
        )


def verify_local_readiness_handoff() -> None:
    require(LOCAL_READINESS.is_file(), "CH-6 local readiness checker is missing")
    require(
        stat.S_IMODE(LOCAL_READINESS.stat().st_mode) == 0o755,
        "CH-6 local readiness checker is not executable mode 0755",
    )
    checker = LOCAL_READINESS.read_text(encoding="utf-8")
    for contract in (
        "ops/stage27/verify-assets.py",
        "ops/stage27/test_recovery_copy.py",
        "adapters::telegram::tests::ch6_",
        "adapters::sqlite::ch6_tests::",
        "--test ch6",
        "--test startup ch6_",
        "--test configuration telegram",
        "delivery_inspection_output_is_bounded_and_redacted",
        "PYTHONDONTWRITEBYTECODE=1",
        "CH6_LOCAL_READINESS=PASS",
    ):
        require(contract in checker, f"CH-6 local checker omits contract: {contract}")
    for forbidden in (
        "aws ",
        "aws\n",
        "systemctl",
        "ssh ",
        "verify-production-host",
        "verify-stage27-linux-boundary",
        "openai_live",
        "xcodebuild",
    ):
        require(forbidden not in checker.lower(), f"CH-6 local checker contains live action: {forbidden}")

    readme = (ASSETS / "README.md").read_text(encoding="utf-8")
    for handoff in (
        "CH-6 local readiness and live handoff",
        "LOCAL VERIFIED",
        "LIVE STILL REQUIRED",
        "scripts/verify-ch6-local",
        "real `getMe`, webhook absence, and long polling",
        "same private Telegram chat",
        "Do not treat local readiness as live acceptance",
    ):
        require(handoff in readme, f"README omits CH-6 handoff: {handoff}")


def verify_canary_absence(canary: str, generated: str) -> None:
    require(canary not in generated, "synthetic token appeared in generated operator output")
    for path in (TEMPLATE, UNIT, ASSETS / "README.md"):
        require(canary not in path.read_text(encoding="utf-8"), f"synthetic token appeared in {path.name}")
    diff = subprocess.run(
        ["git", "diff", "--no-ext-diff", "--", "."],
        cwd=ROOT,
        check=True,
        stdin=subprocess.DEVNULL,
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
        text=True,
    )
    require(canary not in diff.stdout and canary not in diff.stderr, "synthetic token appeared in repository diff")
    repository_files = subprocess.run(
        ["git", "ls-files", "--cached", "--others", "--exclude-standard", "-z"],
        cwd=ROOT,
        check=True,
        stdin=subprocess.DEVNULL,
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
    ).stdout.split(b"\0")
    for relative in repository_files:
        if not relative:
            continue
        path = ROOT / os.fsdecode(relative)
        if path.is_file():
            require(canary.encode() not in path.read_bytes(), f"synthetic token appeared in {path}")
    command_arguments = [
        ["/usr/bin/systemctl", "show", "craxii-server.service", "--property", "ActiveState", "--value"],
        ["/usr/bin/systemctl", "show", "craxii-server.service", "--property", "MainPID", "--value"],
    ]
    require(
        all(canary not in argument for command in command_arguments for argument in command),
        "synthetic token appeared in command arguments",
    )


def main() -> int:
    renderer = load_module("stage27_renderer", RENDERER)
    installer = load_module("stage27_telegram_installer", INSTALLER)
    template_text, enabled_text = verify_configs(renderer)
    verify_systemd(template_text)
    verify_deployment_scripts()
    preflight_canary = verify_upgrade_credential_preflight(installer, renderer)
    canary = verify_installer(installer)
    verify_recovery_assets()
    verify_local_readiness_handoff()
    verify_canary_absence(canary, enabled_text)
    verify_canary_absence(preflight_canary, enabled_text)
    print("STAGE27_CH6_SLICE2_ASSETS=PASS")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
