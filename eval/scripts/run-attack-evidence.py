#!/usr/bin/env python3
from __future__ import annotations

import argparse
import os
import shlex
import shutil
import subprocess
import sys
from datetime import datetime, timezone
from pathlib import Path
from typing import Any

ROOT = Path(__file__).resolve().parents[2]
sys.path.insert(0, str(ROOT / "eval/scripts/lib"))

from attack_evidence import (  # noqa: E402
    build_manifest_entry,
    load_execution,
    resolve_wasmtime_revision,
    sha256_path,
    validate_attack_evidence_bundle,
    validate_attack_manifest,
    validate_attack_receipt,
    write_json,
)

ARTIFACTS = (
    {
        "scenario_id": "healthy-reference",
        "wit_world": "transform-node",
        "manifest": ROOT / "plugins/pass-through/Cargo.toml",
        "artifact": ROOT / "plugins/pass-through/target/wasm32-wasip2/release/wafer_pass_through.wasm",
    },
    {
        "scenario_id": "S1",
        "wit_world": "transform-node",
        "manifest": ROOT / "plugins/attacks/buffer-overflow/Cargo.toml",
        "artifact": ROOT / "plugins/attacks/buffer-overflow/target/wasm32-wasip2/release/wafer_attack_buffer_overflow.wasm",
    },
    {
        "scenario_id": "S2",
        "wit_world": "transform-node",
        "manifest": ROOT / "plugins/attacks/cross-read/Cargo.toml",
        "artifact": ROOT / "plugins/attacks/cross-read/target/wasm32-wasip2/release/wafer_attack_cross_read.wasm",
    },
    {
        "scenario_id": "S3",
        "wit_world": "transform-node",
        "manifest": ROOT / "plugins/attacks/infinite-loop/Cargo.toml",
        "artifact": ROOT / "plugins/attacks/infinite-loop/target/wasm32-wasip2/release/wafer_attack_infinite_loop.wasm",
    },
    {
        "scenario_id": "S4",
        "wit_world": "transform-node",
        "manifest": ROOT / "plugins/attacks/memory-exhaust/Cargo.toml",
        "artifact": ROOT / "plugins/attacks/memory-exhaust/target/wasm32-wasip2/release/wafer_attack_memory_exhaust.wasm",
    },
    {
        "scenario_id": "S5",
        "wit_world": "transform-node",
        "manifest": ROOT / "plugins/attacks/fs-access/Cargo.toml",
        "artifact": ROOT / "plugins/attacks/fs-access/target/wasm32-wasip2/release/wafer_attack_fs_access.wasm",
    },
    {
        "scenario_id": "S6",
        "wit_world": "transform-node",
        "manifest": ROOT / "plugins/attacks/panic/Cargo.toml",
        "artifact": ROOT / "plugins/attacks/panic/target/wasm32-wasip2/release/wafer_attack_panic.wasm",
    },
)


def utc_now() -> str:
    return datetime.now(timezone.utc).isoformat().replace("+00:00", "Z")


def command_string(command: list[str], env: dict[str, str] | None = None) -> str:
    parts: list[str] = []
    if env:
        parts.extend(f"{key}={shlex.quote(value)}" for key, value in sorted(env.items()))
    parts.extend(shlex.quote(part) for part in command)
    return " ".join(parts)


def run_logged(
    command: list[str],
    *,
    log_path: Path,
    env: dict[str, str] | None = None,
) -> subprocess.CompletedProcess[str]:
    full_env = os.environ.copy()
    if env:
        full_env.update(env)
    rendered = command_string(command, env)
    result = subprocess.run(
        command,
        cwd=ROOT,
        env=full_env,
        capture_output=True,
        text=True,
        check=False,
    )
    with log_path.open("a") as stream:
        stream.write(f"$ {rendered}\n")
        if result.stdout:
            stream.write(result.stdout)
            if not result.stdout.endswith("\n"):
                stream.write("\n")
        if result.stderr:
            stream.write(result.stderr)
            if not result.stderr.endswith("\n"):
                stream.write("\n")
        stream.write(f"[exit {result.returncode}]\n")
    if result.returncode != 0:
        raise SystemExit(result.returncode)
    return result


def rustc_verbose() -> str:
    return subprocess.run(["rustc", "-Vv"], cwd=ROOT, capture_output=True, text=True, check=True).stdout.strip()


def active_toolchain() -> str:
    result = subprocess.run(
        ["rustup", "show", "active-toolchain"],
        cwd=ROOT,
        capture_output=True,
        text=True,
        check=True,
    )
    return result.stdout.strip()


def git_sha() -> str:
    return subprocess.run(
        ["git", "rev-parse", "HEAD"],
        cwd=ROOT,
        capture_output=True,
        text=True,
        check=True,
    ).stdout.strip()


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--output-dir", type=Path, required=True)
    parser.add_argument("--source-git-sha")
    parser.add_argument("--release-git-sha")
    return parser.parse_args()


def main() -> int:
    args = parse_args()
    output_dir = args.output_dir.resolve()
    logs_dir = output_dir / "logs"
    logs_dir.mkdir(parents=True, exist_ok=True)

    source_git_sha = args.source_git_sha or git_sha()
    release_git_sha = args.release_git_sha or source_git_sha

    build_log = logs_dir / "build.log"
    validate_log = logs_dir / "validate.log"
    test_log = logs_dir / "test.log"
    manifest_path = output_dir / "attack-manifest.json"
    execution_path = output_dir / "attack-execution.json"
    receipt_path = output_dir / "attack-receipt.json"

    started_at = utc_now()
    shared_target_env = {"CARGO_TARGET_DIR": str(ROOT / "target")}
    for artifact in ARTIFACTS:
        run_logged(
            [
                "cargo",
                "build",
                "--locked",
                "--release",
                "--manifest-path",
                artifact["manifest"].relative_to(ROOT).as_posix(),
                "--target",
                "wasm32-wasip2",
            ],
            log_path=build_log,
            env=shared_target_env,
        )
        built = ROOT / "target/wasm32-wasip2/release" / artifact["artifact"].name
        artifact["artifact"].parent.mkdir(parents=True, exist_ok=True)
        shutil.copy2(built, artifact["artifact"])

    for artifact in ARTIFACTS:
        path = artifact["artifact"]
        if not path.is_file():
            raise SystemExit(f"missing artifact before execution: {path}")
        run_logged(
            ["wasm-tools", "validate", "--features", "component-model", path.relative_to(ROOT).as_posix()],
            log_path=validate_log,
        )

    manifest = {
        "schema_version": 1,
        "generated_at": started_at,
        "source_git_sha": source_git_sha,
        "release_git_sha": release_git_sha,
        "artifacts": [
            build_manifest_entry(
                artifact["scenario_id"],
                artifact["artifact"],
                wit_world=artifact["wit_world"],
                source_git_sha=source_git_sha,
                release_git_sha=release_git_sha,
            )
            for artifact in ARTIFACTS
        ],
    }
    validate_attack_manifest(manifest)
    write_json(manifest_path, manifest)

    test_env = {"WAFER_ATTACK_EVIDENCE_OUTPUT": str(execution_path)}
    test_command = [
        "cargo",
        "test",
        "--locked",
        "-p",
        "wafer-core",
        "--test",
        "attack_containment",
        "mandatory_attack_evidence_receipt",
        "--",
        "--ignored",
        "--exact",
        "--nocapture",
    ]
    run_logged(test_command, log_path=test_log, env=test_env)

    execution = load_execution(execution_path)
    wasmtime = resolve_wasmtime_revision(ROOT / "Cargo.lock")
    finished_at = utc_now()
    receipt: dict[str, Any] = {
        "schema_version": 1,
        "source_git_sha": source_git_sha,
        "release_git_sha": release_git_sha,
        "started_at": started_at,
        "finished_at": finished_at,
        "rustc_version": subprocess.run(
            ["rustc", "--version"],
            cwd=ROOT,
            capture_output=True,
            text=True,
            check=True,
        ).stdout.strip(),
        "rustc_verbose": rustc_verbose(),
        "toolchain": active_toolchain(),
        "wasmtime": wasmtime,
        "test_command": command_string(test_command, test_env),
        "manifest": {
            "path": manifest_path.relative_to(output_dir).as_posix(),
            "sha256": sha256_path(manifest_path),
        },
        "execution": execution,
    }
    validate_attack_receipt(receipt)
    validate_attack_evidence_bundle(
        manifest,
        receipt,
        output_dir=output_dir,
        manifest_path=manifest_path,
    )
    write_json(receipt_path, receipt)
    print(receipt_path)
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
