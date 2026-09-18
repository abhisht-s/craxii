#!/usr/bin/env python3
"""Create, verify, or atomically rotate the Stage 27 Telegram credential."""

from __future__ import annotations

import argparse
import getpass
import os
import pathlib
import pwd
import secrets
import stat
import subprocess
import sys
from collections.abc import Callable


CREDENTIAL_DIRECTORY = pathlib.Path("/etc/craxii/credentials")
CREDENTIAL_NAME = "telegram_bot"
SERVICE = "craxii-server.service"
SERVICE_USER = "craxii-server"


class CredentialError(RuntimeError):
    pass


def _metadata(path: pathlib.Path) -> os.stat_result:
    try:
        return path.lstat()
    except OSError as error:
        raise CredentialError("credential path metadata is unavailable") from error


def _require_directory(path: pathlib.Path, uid: int, gid: int) -> None:
    metadata = _metadata(path)
    if (
        not stat.S_ISDIR(metadata.st_mode)
        or metadata.st_uid != uid
        or metadata.st_gid != gid
        or stat.S_IMODE(metadata.st_mode) != 0o700
    ):
        raise CredentialError("credential directory metadata is not the Stage 27 contract")


def _require_credential(path: pathlib.Path, uid: int, gid: int) -> None:
    metadata = _metadata(path)
    if (
        not stat.S_ISREG(metadata.st_mode)
        or metadata.st_uid != uid
        or metadata.st_gid != gid
        or stat.S_IMODE(metadata.st_mode) != 0o600
        or metadata.st_nlink != 1
        or metadata.st_size == 0
    ):
        raise CredentialError("Telegram credential metadata verification failed")


def verify_credential(directory: pathlib.Path, uid: int, gid: int) -> None:
    """Validate only directory and credential metadata; never open the credential."""
    _require_directory(directory, uid, gid)
    _require_credential(directory / CREDENTIAL_NAME, uid, gid)


def _write_all(descriptor: int, value: bytes) -> None:
    remaining = memoryview(value)
    while remaining:
        written = os.write(descriptor, remaining)
        if written <= 0:
            raise CredentialError("Telegram credential write failed")
        remaining = remaining[written:]


def _temporary_path(directory: pathlib.Path) -> pathlib.Path:
    return directory / f".{CREDENTIAL_NAME}.tmp.{os.getpid()}.{secrets.token_hex(8)}"


def write_credential(
    directory: pathlib.Path,
    secret: bytes,
    uid: int,
    gid: int,
    *,
    rotate: bool,
    replace: Callable[[os.PathLike[str], os.PathLike[str]], None] = os.replace,
) -> None:
    """Write without logging; tests inject only filesystem dependencies."""
    if not secret or any(chr(value).isspace() for value in secret):
        raise CredentialError("empty or whitespace-containing credential refused")

    target = directory / CREDENTIAL_NAME
    if rotate:
        _require_credential(target, uid, gid)
    elif target.exists() or target.is_symlink():
        raise CredentialError("Telegram credential already exists; use explicit rotation")

    temporary = _temporary_path(directory)
    descriptor = -1
    try:
        flags = os.O_WRONLY | os.O_CREAT | os.O_EXCL
        flags |= getattr(os, "O_CLOEXEC", 0)
        flags |= getattr(os, "O_NOFOLLOW", 0)
        descriptor = os.open(temporary, flags, 0o600)
        os.fchmod(descriptor, 0o600)
        os.fchown(descriptor, uid, gid)
        _write_all(descriptor, secret)
        os.fsync(descriptor)
        os.close(descriptor)
        descriptor = -1

        if rotate:
            replace(temporary, target)
        else:
            try:
                os.link(temporary, target, follow_symlinks=False)
            except FileExistsError as error:
                raise CredentialError(
                    "Telegram credential appeared during installation; nothing was overwritten"
                ) from error
            temporary.unlink()

        directory_descriptor = os.open(
            directory,
            os.O_RDONLY
            | getattr(os, "O_DIRECTORY", 0)
            | getattr(os, "O_CLOEXEC", 0),
        )
        try:
            os.fsync(directory_descriptor)
        finally:
            os.close(directory_descriptor)
        _require_credential(target, uid, gid)
    finally:
        if descriptor >= 0:
            os.close(descriptor)
        try:
            temporary.unlink()
        except FileNotFoundError:
            pass


def _service_is_stopped() -> bool:
    active = subprocess.run(
        ["/usr/bin/systemctl", "show", SERVICE, "--property", "ActiveState", "--value"],
        check=False,
        stdin=subprocess.DEVNULL,
        stdout=subprocess.PIPE,
        stderr=subprocess.DEVNULL,
        text=True,
    )
    main_pid = subprocess.run(
        ["/usr/bin/systemctl", "show", SERVICE, "--property", "MainPID", "--value"],
        check=False,
        stdin=subprocess.DEVNULL,
        stdout=subprocess.PIPE,
        stderr=subprocess.DEVNULL,
        text=True,
    )
    return (
        active.returncode == 0
        and active.stdout.strip() in {"inactive", "failed"}
        and main_pid.returncode == 0
        and main_pid.stdout.strip() == "0"
    )


def _arguments() -> argparse.Namespace:
    parser = argparse.ArgumentParser()
    operation = parser.add_mutually_exclusive_group(required=True)
    operation.add_argument("--install", action="store_true")
    operation.add_argument("--rotate", action="store_true")
    operation.add_argument("--verify", action="store_true")
    return parser.parse_args()


def main() -> int:
    arguments = _arguments()
    try:
        if os.geteuid() != 0:
            raise CredentialError("run as root on the explicitly authorized Stage 27 host")
        identity = pwd.getpwnam(SERVICE_USER)
        if arguments.verify:
            verify_credential(CREDENTIAL_DIRECTORY, identity.pw_uid, identity.pw_gid)
            print("Telegram credential metadata verified; credential contents were not read.")
            return 0

        _require_directory(CREDENTIAL_DIRECTORY, identity.pw_uid, identity.pw_gid)
        if not sys.stdin.isatty():
            raise CredentialError("Telegram credential input requires an interactive terminal")
        if arguments.rotate and not _service_is_stopped():
            raise CredentialError("craxii-server.service must be stopped with no live MainPID")

        value = getpass.getpass("Telegram bot token (input hidden): ")
        encoded = b""
        try:
            encoded = value.encode("utf-8")
            write_credential(
                CREDENTIAL_DIRECTORY,
                encoded,
                identity.pw_uid,
                identity.pw_gid,
                rotate=arguments.rotate,
            )
        finally:
            value = ""
            encoded = b""

        operation = "rotated" if arguments.rotate else "installed"
        service_result = (
            "service remains stopped"
            if arguments.rotate
            else "service state was not changed"
        )
        print(
            f"Telegram credential {operation} with verified metadata; "
            f"{service_result} and credential contents were not printed."
        )
        return 0
    except (CredentialError, KeyError, OSError, UnicodeError) as error:
        print(f"error: {error}", file=sys.stderr)
        return 1


if __name__ == "__main__":
    raise SystemExit(main())
