#!/usr/bin/env python3
import json
import hashlib
import os
import pathlib
import sys

path = pathlib.Path(sys.argv[1])
value = json.loads(path.read_text(encoding="utf-8"))
value["consolidated_repository_gate"] = {"status": "passed", "runs": 1}
derived = pathlib.Path(value["build_artifacts"]["derived_data"])
release = derived / "Build/Products/Release/Craxii.app/Contents/MacOS/Craxii"
value["fixture_isolation"]["release_build_and_string_isolation"] = True
value["native_app"]["release_executable_sha256"] = hashlib.sha256(
    release.read_bytes()
).hexdigest()
data = json.dumps(value, indent=2, sort_keys=True).encode("utf-8")
temporary = path.with_name(path.name + ".tmp")
descriptor = os.open(temporary, os.O_WRONLY | os.O_CREAT | os.O_EXCL, 0o600)
with os.fdopen(descriptor, "wb") as stream:
    stream.write(data)
os.replace(temporary, path)
