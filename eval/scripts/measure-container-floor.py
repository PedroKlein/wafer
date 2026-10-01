#!/usr/bin/env python3
"""Build and measure the E-Density-1 container floor.

The floor is a ``FROM scratch`` image whose only file is the statically linked
Rust pass-through worker in eval/container-floor/worker, built with the
plugins' toolchain and release profile inside a digest-pinned
``rust:<toolchain>-alpine`` image. The sizes are read from the image that
``docker save`` exports, so they do not depend on how the local daemon stores
images. Run it on a machine with Docker that can run images for the target
platform, then commit the JSON it writes:

    measure-container-floor.py --platform linux/arm64
"""

from __future__ import annotations

import argparse
import datetime as dt
import gzip
import hashlib
import io
import json
import re
import subprocess
import sys
import tarfile
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
FLOOR_DIR = ROOT / "eval/container-floor"
INPUTS = ("Dockerfile", "worker/Cargo.toml", "worker/Cargo.lock", "worker/src/main.rs")
PLATFORMS = ("linux/arm64", "linux/amd64")
SMOKE_INPUT = b'{"temperature": 72.5}\n'


def docker(*args: str, stdin: bytes | None = None) -> bytes:
    completed = subprocess.run(
        ["docker", *args], input=stdin, capture_output=True, check=False
    )
    if completed.returncode != 0:
        detail = completed.stderr.decode(errors="replace").strip()
        raise RuntimeError(f"docker {' '.join(args[:2])} failed: {detail}")
    return completed.stdout


def input_hashes(floor_dir: Path = FLOOR_DIR) -> dict[str, str]:
    return {
        name: hashlib.sha256((floor_dir / name).read_bytes()).hexdigest() for name in INPUTS
    }


def rust_toolchain(root: Path = ROOT) -> str:
    match = re.search(
        r'^channel\s*=\s*"([^"]+)"', (root / "rust-toolchain.toml").read_text(), re.MULTILINE
    )
    if match is None:
        raise ValueError("rust-toolchain.toml has no channel")
    return match.group(1)


def pinned_build_image(toolchain: str, platform: str) -> str:
    image = f"rust:{toolchain}-alpine"
    docker("pull", "--quiet", "--platform", platform, image)
    digests = json.loads(docker("image", "inspect", "--format", "{{json .RepoDigests}}", image))
    for reference in digests or []:
        name, _, digest = reference.partition("@")
        if name in {"rust", "docker.io/library/rust"} and re.fullmatch(
            r"sha256:[0-9a-f]{64}", digest
        ):
            return f"{image}@{digest}"
    raise RuntimeError(f"{image} has no registry digest to pin the build to")


def measure_saved_image(archive: bytes, platform: str) -> dict:
    """Size of the one platform image in a ``docker save`` archive."""
    with tarfile.open(fileobj=io.BytesIO(archive)) as saved:
        images = []
        for entry in json.load(saved.extractfile("manifest.json")):
            config = json.load(saved.extractfile(entry["Config"]))
            if f"{config.get('os')}/{config.get('architecture')}" == platform:
                layers = [saved.extractfile(name).read() for name in entry["Layers"]]
                images.append((config, layers))
    if len(images) != 1:
        raise ValueError(f"expected one {platform} image in the archive, found {len(images)}")
    config, layers = images[0]
    diff_ids = config.get("rootfs", {}).get("diff_ids", [])
    if len(layers) != 1 or len(diff_ids) != 1:
        raise ValueError(f"a FROM scratch floor has one layer, found {len(layers)}")
    layer = layers[0]
    if layer[:2] == b"\x1f\x8b":
        layer = gzip.decompress(layer)
    diff_id = f"sha256:{hashlib.sha256(layer).hexdigest()}"
    if diff_id != diff_ids[0]:
        raise ValueError("the layer does not match the diff_id in the image config")
    with tarfile.open(fileobj=io.BytesIO(layer)) as contents:
        entries = [member for member in contents.getmembers() if not member.isdir()]
    names = [member.name.removeprefix("./") for member in entries if member.isfile()]
    if len(entries) != 1 or names != ["worker"]:
        raise ValueError("the floor image must hold exactly one file, /worker")
    if config.get("config", {}).get("Entrypoint") != ["/worker"]:
        raise ValueError("the floor image must run /worker")
    return {
        "layer_count": 1,
        "layer_diff_id": diff_id,
        "image_bytes": len(layer),
        "binary_bytes": entries[0].size,
    }


def measure(platform: str, root: Path = ROOT) -> dict:
    floor_dir = root / "eval/container-floor"
    toolchain = rust_toolchain(root)
    build_image = pinned_build_image(toolchain, platform)
    tag = f"wafer-container-floor:{platform.replace('/', '-')}"
    docker(
        "build",
        "--platform",
        platform,
        "--build-arg",
        f"BUILD_IMAGE={build_image}",
        "--tag",
        tag,
        str(floor_dir),
    )
    echoed = docker(
        "run",
        "--rm",
        "--interactive",
        "--network",
        "none",
        "--platform",
        platform,
        tag,
        stdin=SMOKE_INPUT,
    )
    if echoed != SMOKE_INPUT:
        raise RuntimeError("the floor image did not pass its input through unchanged")
    image = measure_saved_image(docker("save", tag), platform)
    return {
        "schema_version": 1,
        "base": "scratch",
        "platform": platform,
        "build_image": build_image,
        "rust_toolchain": toolchain,
        "inputs_sha256": input_hashes(floor_dir),
        "smoke_test": "passed stdin through unchanged",
        **image,
        "docker_server_version": docker("version", "--format", "{{.Server.Version}}")
        .decode()
        .strip(),
        "measured_at": dt.datetime.now(dt.timezone.utc).strftime("%Y-%m-%dT%H:%M:%SZ"),
    }


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    parser.add_argument("--platform", choices=PLATFORMS, default="linux/arm64")
    parser.add_argument(
        "--output",
        type=Path,
        help="default: eval/container-floor/<os>-<arch>.json for the platform",
    )
    args = parser.parse_args()
    output = args.output or FLOOR_DIR / f"{args.platform.replace('/', '-')}.json"
    try:
        record = measure(args.platform)
    except (OSError, RuntimeError, ValueError, KeyError, tarfile.TarError) as error:
        print(f"error: {error}", file=sys.stderr)
        return 1
    output.write_text(json.dumps(record, indent=2) + "\n")
    print(
        f"Wrote {output}: image {record['image_bytes']} bytes, "
        f"worker {record['binary_bytes']} bytes"
    )
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
