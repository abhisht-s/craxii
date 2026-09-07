#!/usr/bin/env python3
"""Create a scoped Stage 26 UI-test run configuration without embedding secrets."""

from __future__ import annotations

import argparse
import os
import pathlib
import plistlib
import tempfile
import urllib.parse


STAGE26_VARIABLES = {
    "CRAXII_STAGE26_LIVE",
    "CRAXII_STAGE26_CANCELLATION",
    "CRAXII_STAGE26_CONTROL_URL",
}


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--derived", type=pathlib.Path, required=True)
    parser.add_argument("--mode", choices=["live", "cancellation"], required=True)
    parser.add_argument("--control-url", required=True)
    args = parser.parse_args()

    parsed = urllib.parse.urlsplit(args.control_url)
    if parsed.scheme != "http" or parsed.hostname != "127.0.0.1" or not parsed.port:
        raise RuntimeError("Stage 26 controller must be an explicit loopback HTTP URL")

    products = args.derived / "Build" / "Products"
    sources = sorted(
        path for path in products.glob("*.xctestrun")
        if not path.name.startswith(".stage26-")
    )
    if len(sources) != 1:
        raise RuntimeError("expected exactly one built Craxii xctestrun configuration")
    with sources[0].open("rb") as stream:
        configuration = plistlib.load(stream)

    test_entries = [
        value for value in configuration.values()
        if isinstance(value, dict) and value.get("BlueprintName") == "CraxiiUITests"
    ]
    if len(test_entries) != 1:
        raise RuntimeError("Craxii UI-test configuration is missing or ambiguous")
    environment = test_entries[0].setdefault("EnvironmentVariables", {})
    for name in STAGE26_VARIABLES:
        environment.pop(name, None)
    environment[
        "CRAXII_STAGE26_LIVE"
        if args.mode == "live"
        else "CRAXII_STAGE26_CANCELLATION"
    ] = "1"
    environment["CRAXII_STAGE26_CONTROL_URL"] = args.control_url

    descriptor, name = tempfile.mkstemp(
        prefix=".stage26-", suffix=".xctestrun", dir=products)
    try:
        os.fchmod(descriptor, 0o600)
        with os.fdopen(descriptor, "wb") as stream:
            plistlib.dump(configuration, stream, fmt=plistlib.FMT_BINARY)
    except Exception:
        pathlib.Path(name).unlink(missing_ok=True)
        raise
    print(name)
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
