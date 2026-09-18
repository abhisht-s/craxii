#!/usr/bin/env python3
"""Render Stage 27 config while keeping Telegram host values host-owned."""

from __future__ import annotations

import argparse
import json
import os
import pathlib
import re
import sys
import tomllib
from typing import Any


OPENAI_CREDENTIAL = "openai_provider"
TELEGRAM_CREDENTIAL = "telegram_bot"
TELEGRAM_KEYS = {
    "enabled",
    "channel_account_id",
    "credential",
    "expected_bot_user_id",
    "owner_telegram_user_id",
}


class RenderError(ValueError):
    pass


def _load(path: pathlib.Path) -> tuple[str, dict[str, Any]]:
    try:
        source = path.read_text(encoding="utf-8")
        parsed = tomllib.loads(source)
    except (OSError, UnicodeError, tomllib.TOMLDecodeError) as error:
        raise RenderError(f"cannot parse configuration: {path}") from error
    if not isinstance(parsed, dict):
        raise RenderError("configuration root is not a table")
    return source, parsed


def _telegram_from(parsed: dict[str, Any]) -> dict[str, Any]:
    telegram = parsed.get("telegram", {"enabled": False})
    if not isinstance(telegram, dict) or set(telegram) - TELEGRAM_KEYS:
        raise RenderError("Telegram configuration has an unsupported shape")
    enabled = telegram.get("enabled")
    if type(enabled) is not bool:
        raise RenderError("telegram.enabled must be a Boolean")
    if not enabled:
        if telegram != {"enabled": False}:
            raise RenderError("disabled Telegram configuration contains provider fields")
        return {"enabled": False}

    expected_keys = TELEGRAM_KEYS
    if set(telegram) != expected_keys:
        raise RenderError("enabled Telegram configuration is incomplete")
    if telegram["credential"] != TELEGRAM_CREDENTIAL:
        raise RenderError("Telegram credential name must be telegram_bot")
    if not isinstance(telegram["channel_account_id"], str):
        raise RenderError("Telegram channel account ID must be a string")
    for field in ("expected_bot_user_id", "owner_telegram_user_id"):
        if type(telegram[field]) is not int:
            raise RenderError(f"{field} must be an integer")
    return dict(telegram)


def _enabled_from_arguments(arguments: argparse.Namespace) -> dict[str, Any]:
    values = (
        arguments.channel_account_id,
        arguments.expected_bot_user_id,
        arguments.owner_telegram_user_id,
    )
    if any(value is None for value in values):
        raise RenderError("enabling Telegram requires the channel account, bot, and owner IDs")
    return {
        "enabled": True,
        "channel_account_id": arguments.channel_account_id,
        "credential": TELEGRAM_CREDENTIAL,
        "expected_bot_user_id": arguments.expected_bot_user_id,
        "owner_telegram_user_id": arguments.owner_telegram_user_id,
    }


def _table_range(source: str, name: str) -> tuple[int, int]:
    heading = re.compile(rf"(?m)^\[{re.escape(name)}\][ \t]*$")
    match = heading.search(source)
    if match is None:
        raise RenderError(f"missing [{name}] table")
    next_heading = re.search(r"(?m)^\[", source[match.end() :])
    end = len(source) if next_heading is None else match.end() + next_heading.start()
    return match.start(), end


def _replace_declared(source: str, enabled: bool) -> str:
    start, end = _table_range(source, "credentials")
    table = source[start:end]
    declaration = re.compile(r"(?m)^declared[ \t]*=[ \t]*.*$")
    matches = list(declaration.finditer(table))
    if len(matches) != 1:
        raise RenderError("credentials.declared must have one source line")
    credentials = [OPENAI_CREDENTIAL]
    if enabled:
        credentials.append(TELEGRAM_CREDENTIAL)
    replacement = "declared = [" + ", ".join(json.dumps(value) for value in credentials) + "]"
    table = declaration.sub(replacement, table, count=1)
    return source[:start] + table + source[end:]


def _replace_telegram(source: str, telegram: dict[str, Any]) -> str:
    start, end = _table_range(source, "telegram")
    lines = ["[telegram]", f"enabled = {str(telegram['enabled']).lower()}"]
    if telegram["enabled"]:
        lines.extend(
            [
                f"channel_account_id = {json.dumps(telegram['channel_account_id'])}",
                f"credential = {json.dumps(TELEGRAM_CREDENTIAL)}",
                f"expected_bot_user_id = {telegram['expected_bot_user_id']}",
                f"owner_telegram_user_id = {telegram['owner_telegram_user_id']}",
            ]
        )
    block = "\n".join(lines) + "\n\n"
    return source[:start] + block + source[end:].lstrip("\n")


def render(source: str, telegram: dict[str, Any]) -> str:
    parsed = tomllib.loads(source)
    credentials = parsed.get("credentials")
    if not isinstance(credentials, dict) or credentials.get("source") != "systemd":
        raise RenderError("Stage 27 requires the systemd credential source")
    declared = credentials.get("declared")
    legal_sets = ([OPENAI_CREDENTIAL], [OPENAI_CREDENTIAL, TELEGRAM_CREDENTIAL])
    if declared not in legal_sets:
        raise RenderError("Stage 27 template has an unsupported credential declaration")
    _telegram_from(parsed)
    rendered = _replace_telegram(_replace_declared(source, telegram["enabled"]), telegram)
    if "__REQUIRED_" in rendered:
        raise RenderError("rendered configuration contains an unresolved placeholder")
    # This is a structural guard only. The candidate Rust binary remains authoritative for all
    # CH-5 semantic validation, including UUIDv7 and Telegram identifier ranges.
    reparsed = tomllib.loads(rendered)
    if _telegram_from(reparsed) != telegram:
        raise RenderError("rendered Telegram configuration did not round-trip")
    return rendered


def _write_new(path: pathlib.Path, content: str) -> None:
    flags = os.O_WRONLY | os.O_CREAT | os.O_EXCL
    flags |= getattr(os, "O_CLOEXEC", 0)
    flags |= getattr(os, "O_NOFOLLOW", 0)
    descriptor = os.open(path, flags, 0o600)
    try:
        data = content.encode("utf-8")
        while data:
            written = os.write(descriptor, data)
            data = data[written:]
        os.fsync(descriptor)
    finally:
        os.close(descriptor)


def _arguments() -> argparse.Namespace:
    parser = argparse.ArgumentParser()
    parser.add_argument("--template", required=True, type=pathlib.Path)
    selection = parser.add_mutually_exclusive_group()
    selection.add_argument("--preserve-telegram-from", type=pathlib.Path)
    selection.add_argument("--enable-telegram", action="store_true")
    selection.add_argument("--disable-telegram", action="store_true")
    parser.add_argument("--channel-account-id")
    parser.add_argument("--expected-bot-user-id", type=int)
    parser.add_argument("--owner-telegram-user-id", type=int)
    destination = parser.add_mutually_exclusive_group(required=True)
    destination.add_argument("--output", type=pathlib.Path)
    destination.add_argument("--check", type=pathlib.Path)
    return parser.parse_args()


def main() -> int:
    arguments = _arguments()
    try:
        source, parsed = _load(arguments.template)
        if arguments.preserve_telegram_from is not None:
            _, preserved = _load(arguments.preserve_telegram_from)
            telegram = _telegram_from(preserved)
        elif arguments.enable_telegram:
            telegram = _enabled_from_arguments(arguments)
        elif arguments.disable_telegram:
            telegram = {"enabled": False}
        else:
            telegram = _telegram_from(parsed)

        if not arguments.enable_telegram and any(
            value is not None
            for value in (
                arguments.channel_account_id,
                arguments.expected_bot_user_id,
                arguments.owner_telegram_user_id,
            )
        ):
            raise RenderError("Telegram identity values require --enable-telegram")

        rendered = render(source, telegram)
        if arguments.output is not None:
            _write_new(arguments.output, rendered)
        else:
            existing = arguments.check.read_text(encoding="utf-8")
            if existing != rendered:
                raise RenderError("deployed config differs from the rendered candidate")
        return 0
    except (OSError, UnicodeError, tomllib.TOMLDecodeError, RenderError) as error:
        print(f"error: {error}", file=sys.stderr)
        return 1


if __name__ == "__main__":
    raise SystemExit(main())
