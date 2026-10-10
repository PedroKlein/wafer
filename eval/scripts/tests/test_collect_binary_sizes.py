#!/usr/bin/env python3

import csv
import json
import os
import platform
import shutil
import subprocess
from pathlib import Path

ROOT = Path(__file__).resolve().parents[3]
FLOOR_NAME = "linux-{}.json".format(
    {"aarch64": "arm64", "arm64": "arm64", "x86_64": "amd64", "amd64": "amd64"}[
        platform.machine().lower()
    ]
)


def make_tree(root: Path) -> Path:
    scripts = root / "eval/scripts"
    scripts.mkdir(parents=True)
    shutil.copy2(ROOT / "eval/scripts/collect-binary-sizes.sh", scripts)
    (scripts / "binary-sizes.index").write_text(
        "# Format: name|wasm_path\n"
        "pass-through|plugins/pass-through/pass_through.wasm\n"
        "json-parse|plugins/json-parse/json_parse.wasm\n"
    )
    for name, size in (("pass-through", 2048), ("json-parse", 3072)):
        plugin = root / "plugins" / name
        plugin.mkdir(parents=True)
        (plugin / f"{name.replace('-', '_')}.wasm").write_bytes(b"\0" * size)
    return scripts / "collect-binary-sizes.sh"


def test_writes_component_sizes_and_copies_the_measured_floor(tmp_path: Path) -> None:
    script = make_tree(tmp_path)
    floor = tmp_path / "eval/container-floor" / FLOOR_NAME
    floor.parent.mkdir(parents=True)
    floor.write_text(json.dumps({"base": "scratch", "image_bytes": 400_000}) + "\n")
    output = tmp_path / "leaf"

    completed = subprocess.run(
        [str(script), str(output)], capture_output=True, text=True, check=False
    )

    assert completed.returncode == 0, completed.stderr
    with (output / "binary-sizes.csv").open(newline="") as stream:
        rows = list(csv.DictReader(stream))
    assert rows == [
        {"plugin": "pass-through", "wasm_bytes": "2048", "wasm_kb": "2.0"},
        {"plugin": "json-parse", "wasm_bytes": "3072", "wasm_kb": "3.0"},
    ]
    assert (output / "container-floor.json").read_bytes() == floor.read_bytes()


def test_refuses_to_run_without_a_measured_floor(tmp_path: Path) -> None:
    script = make_tree(tmp_path)
    output = tmp_path / "leaf"

    completed = subprocess.run(
        [str(script), str(output)], capture_output=True, text=True, check=False
    )

    assert completed.returncode == 1
    assert f"missing eval/container-floor/{FLOOR_NAME}" in completed.stderr
    assert "measure-container-floor.py" in completed.stderr
    assert not (output / "binary-sizes.csv").exists()


def test_formats_kilobytes_in_the_c_locale(tmp_path: Path) -> None:
    script = make_tree(tmp_path)
    floor = tmp_path / "eval/container-floor" / FLOOR_NAME
    floor.parent.mkdir(parents=True)
    floor.write_text(json.dumps({"base": "scratch", "image_bytes": 400_000}) + "\n")
    shim = tmp_path / "bin"
    shim.mkdir()
    (shim / "awk").write_text(
        f'#!/bin/sh\n[ "$LC_ALL" = C ] || exit 7\nexec {shutil.which("awk")} "$@"\n'
    )
    (shim / "awk").chmod(0o755)
    env = {**os.environ, "PATH": f"{shim}:{os.environ['PATH']}", "LC_ALL": "de_DE.UTF-8"}

    completed = subprocess.run(
        [str(script), str(tmp_path / "leaf")], capture_output=True, text=True, check=False, env=env
    )

    assert completed.returncode == 0, completed.stderr
    rows = (tmp_path / "leaf/binary-sizes.csv").read_text().splitlines()
    assert rows[1:] == ["pass-through,2048,2.0", "json-parse,3072,3.0"]
