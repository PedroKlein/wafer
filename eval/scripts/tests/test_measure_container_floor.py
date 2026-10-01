#!/usr/bin/env python3

import gzip
import hashlib
import importlib.util
import io
import json
import os
import subprocess
import sys
import tarfile
from pathlib import Path

import pytest

ROOT = Path(__file__).resolve().parents[3]
TOOL = ROOT / "eval/scripts/measure-container-floor.py"

spec = importlib.util.spec_from_file_location("measure_container_floor", TOOL)
assert spec and spec.loader
measure_container_floor = importlib.util.module_from_spec(spec)
spec.loader.exec_module(measure_container_floor)

PINNED_DIGEST = "sha256:" + "a" * 64
FAKE_DOCKER = f"""#!{sys.executable}
import json, os, sys
args = sys.argv[1:]
with open(os.environ["FAKE_DOCKER_LOG"], "a") as log:
    log.write(json.dumps(args) + "\\n")
if args[0] == "pull":
    print("docker.io/library/" + args[-1])
elif args[:2] == ["image", "inspect"]:
    print(json.dumps(["rust@{PINNED_DIGEST}"]))
elif args[0] == "build":
    pass
elif args[0] == "run":
    data = sys.stdin.buffer.read()
    sys.stdout.buffer.write(b"exec format error\\n" if os.environ.get("FAKE_DOCKER_RUN") == "broken" else data)
elif args[0] == "save":
    sys.stdout.buffer.write(open(os.environ["FAKE_DOCKER_SAVE"], "rb").read())
elif args[0] == "version":
    print("29.0.0")
else:
    sys.exit("unexpected docker call: " + " ".join(args))
"""
WORKER = b"\x7fELF" + b"\x00" * 4096


def sha256(data: bytes) -> str:
    return hashlib.sha256(data).hexdigest()


def write_saved_image(
    path: Path,
    *,
    architecture: str = "arm64",
    files: dict[str, bytes] | None = None,
    layer_count: int = 1,
    compress: bool = False,
    entrypoint: list[str] | None = None,
) -> int:
    """Write a ``docker save`` archive and return the uncompressed layer size."""
    buffer = io.BytesIO()
    with tarfile.open(fileobj=buffer, mode="w") as layer:
        for name, data in (files or {"worker": WORKER}).items():
            info = tarfile.TarInfo(name)
            info.size = len(data)
            info.mode = 0o755
            layer.addfile(info, io.BytesIO(data))
    layer_tar = buffer.getvalue()
    blob = gzip.compress(layer_tar) if compress else layer_tar
    config = json.dumps(
        {
            "architecture": architecture,
            "os": "linux",
            "config": {"Entrypoint": entrypoint or ["/worker"]},
            "rootfs": {"type": "layers", "diff_ids": [f"sha256:{sha256(layer_tar)}"] * layer_count},
        }
    ).encode()
    manifest = json.dumps(
        [
            {
                "Config": f"blobs/sha256/{sha256(config)}",
                "RepoTags": ["wafer-container-floor:linux-arm64"],
                "Layers": [f"blobs/sha256/{sha256(blob)}"] * layer_count,
            }
        ]
    ).encode()
    with tarfile.open(path, mode="w") as archive:
        for name, data in (
            ("manifest.json", manifest),
            (f"blobs/sha256/{sha256(config)}", config),
            (f"blobs/sha256/{sha256(blob)}", blob),
        ):
            info = tarfile.TarInfo(name)
            info.size = len(data)
            archive.addfile(info, io.BytesIO(data))
    return len(layer_tar)


def run_tool(
    tmp_path: Path, saved: Path, **env: str
) -> tuple[subprocess.CompletedProcess[str], Path, list[list[str]]]:
    bin_dir = tmp_path / "bin"
    bin_dir.mkdir(exist_ok=True)
    (bin_dir / "docker").write_text(FAKE_DOCKER)
    (bin_dir / "docker").chmod(0o755)
    log = tmp_path / "docker.log"
    output = tmp_path / "floor.json"
    completed = subprocess.run(
        [sys.executable, str(TOOL), "--platform", "linux/arm64", "--output", str(output)],
        cwd=tmp_path,
        env={
            **os.environ,
            "PATH": f"{bin_dir}:{os.environ['PATH']}",
            "FAKE_DOCKER_LOG": str(log),
            "FAKE_DOCKER_SAVE": str(saved),
            **env,
        },
        capture_output=True,
        text=True,
        check=False,
    )
    calls = [json.loads(line) for line in log.read_text().splitlines()] if log.exists() else []
    return completed, output, calls


@pytest.mark.parametrize("compress", [False, True])
def test_records_the_measured_scratch_image_and_its_build_inputs(
    tmp_path: Path, compress: bool
) -> None:
    saved = tmp_path / "saved.tar"
    layer_bytes = write_saved_image(saved, compress=compress)

    completed, output, calls = run_tool(tmp_path, saved)

    assert completed.returncode == 0, completed.stderr
    record = json.loads(output.read_text())
    toolchain = measure_container_floor.rust_toolchain()
    build_image = f"rust:{toolchain}-alpine@{PINNED_DIGEST}"
    assert record["base"] == "scratch"
    assert record["platform"] == "linux/arm64"
    assert record["image_bytes"] == layer_bytes
    assert record["binary_bytes"] == len(WORKER)
    assert record["layer_count"] == 1
    assert record["build_image"] == build_image
    assert record["rust_toolchain"] == toolchain
    assert record["inputs_sha256"] == {
        name: sha256((ROOT / "eval/container-floor" / name).read_bytes())
        for name in measure_container_floor.INPUTS
    }
    assert record["docker_server_version"] == "29.0.0"
    build = next(call for call in calls if call[0] == "build")
    assert f"BUILD_IMAGE={build_image}" in build
    assert build[build.index("--network") + 1] == "none"
    assert build[-1] == str(ROOT / "eval/container-floor")
    assert any(call[0] == "run" and "none" in call for call in calls)


@pytest.mark.parametrize(
    ("image", "message"),
    [
        ({"files": {"worker": WORKER, "etc/passwd": b"root\n"}}, "exactly one file"),
        ({"layer_count": 2}, "one layer"),
        ({"architecture": "amd64"}, "one linux/arm64 image"),
        ({"entrypoint": ["/bin/sh"]}, "must run /worker"),
    ],
)
def test_rejects_an_image_that_is_not_the_scratch_floor(
    tmp_path: Path, image: dict, message: str
) -> None:
    saved = tmp_path / "saved.tar"
    write_saved_image(saved, **image)

    completed, output, _ = run_tool(tmp_path, saved)

    assert completed.returncode == 1
    assert message in completed.stderr
    assert not output.exists()


def test_rejects_an_image_that_does_not_pass_its_input_through(tmp_path: Path) -> None:
    saved = tmp_path / "saved.tar"
    write_saved_image(saved)

    completed, output, calls = run_tool(tmp_path, saved, FAKE_DOCKER_RUN="broken")

    assert completed.returncode == 1
    assert "did not pass its input through unchanged" in completed.stderr
    assert not output.exists()
    assert not any(call[0] == "save" for call in calls)


def test_committed_floors_match_their_build_inputs() -> None:
    hashes = measure_container_floor.input_hashes()
    toolchain = measure_container_floor.rust_toolchain()
    for path in sorted((ROOT / "eval/container-floor").glob("*.json")):
        record = json.loads(path.read_text())
        assert record["platform"] == path.stem.replace("-", "/", 1), path.name
        assert record["inputs_sha256"] == hashes, f"{path.name} is stale: measure it again"
        assert record["rust_toolchain"] == toolchain, f"{path.name} is stale: measure it again"
