#!/usr/bin/env python3
import hashlib
import json
import pathlib
import sys

manifest_path = pathlib.Path(sys.argv[1])
artifact_dir = pathlib.Path(sys.argv[2])
manifest = json.loads(manifest_path.read_text())
required = {
    "schema_version",
    "source_commit",
    "wasmtime_revision",
    "rust_version",
    "wit_bindgen_version",
    "wasip3_version",
    "wasip3_crate_sha256",
    "wasm_tools_version",
    "wit_sha256",
    "artifact_sha256",
    "build_target",
    "commands",
}
assert set(manifest) == required
assert manifest["schema_version"] == 1
assert len(manifest["source_commit"]) == 40
assert len(manifest["wasmtime_revision"]) == 40
assert manifest["commands"]

for name, expected in manifest["artifact_sha256"].items():
    path = artifact_dir / ("host/debug/wafer-p3-host" if name == "wafer-p3-host" else f"artifacts/{name}")
    actual = hashlib.sha256(path.read_bytes()).hexdigest()
    assert actual == expected, f"{path}: {actual} != {expected}"

wit_dir = pathlib.Path("experiments/p3/wit/wafer-pipeline-0.2.0")
for name, expected in manifest["wit_sha256"].items():
    actual = hashlib.sha256((wit_dir / name).read_bytes()).hexdigest()
    assert actual == expected, f"{name}: {actual} != {expected}"

print("fixture_manifest_valid=true")
