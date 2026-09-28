#!/usr/bin/env python3

from __future__ import annotations

import argparse
import csv
import hashlib
import json
import platform
import re
import subprocess
import sys
import tomllib
from datetime import datetime, timezone
from pathlib import Path
from typing import Any

SCHEMA = "wafer-p2-async-ab-run-v1"
BATCH_SCHEMA = "wafer-p2-async-ab-batch-v1"
WASMTIME_REVISION = "e9f1ea232fd245aea338ab3eb7d73487ae75cab1"
CONDITIONS = {"120b": 120, "1kb": 1024, "100kb": 102400}
REQUIRED_FILES = (
    "config.toml",
    "metadata.json",
    "runtime-provenance.json",
    "latency.hdr",
    "latency-summary.json",
    "throughput.csv",
    "memory.csv",
    "memory-clock.json",
    "measurement-window.json",
    "sequence.csv",
    "stdout.log",
)


def _read_json(path: Path) -> dict[str, Any]:
    value = json.loads(path.read_text())
    if not isinstance(value, dict):
        raise ValueError(f"{path} must contain a JSON object")
    return value


def _sha256(path: Path) -> str:
    digest = hashlib.sha256()
    with path.open("rb") as stream:
        for chunk in iter(lambda: stream.read(1024 * 1024), b""):
            digest.update(chunk)
    return digest.hexdigest()


def _command(*args: str, cwd: Path | None = None) -> str:
    return subprocess.check_output(args, cwd=cwd, text=True).strip()


def _tree_hash(root: Path) -> str:
    digest = hashlib.sha256()
    for path in sorted(item for item in root.rglob("*") if item.is_file()):
        digest.update(path.relative_to(root).as_posix().encode())
        digest.update(b"\0")
        digest.update(path.read_bytes())
    return digest.hexdigest()


def _csv_rows(path: Path) -> list[dict[str, str]]:
    with path.open(newline="") as stream:
        return list(csv.DictReader(stream))


def validate_run_outputs(leaf: Path, expected_sha: str, payload_bytes: int) -> dict[str, Any]:
    for name in REQUIRED_FILES:
        path = leaf / name
        if not path.is_file() or path.stat().st_size == 0:
            raise ValueError(f"missing or empty run artifact: {path}")

    metadata = _read_json(leaf / "metadata.json")
    provenance = _read_json(leaf / "runtime-provenance.json")
    summary = _read_json(leaf / "latency-summary.json")
    window = _read_json(leaf / "measurement-window.json")
    memory_clock = _read_json(leaf / "memory-clock.json")
    with (leaf / "config.toml").open("rb") as stream:
        config = tomllib.load(stream)

    if metadata.get("git_sha") != expected_sha or metadata.get("git_dirty") is not False:
        raise ValueError("metadata source identity is not the expected clean revision")
    if metadata.get("host_tag") != "shakedown-macos":
        raise ValueError("P2 A/B runs must use the shakedown-macos host tag")
    if metadata.get("exit_codes", {}).get("wafer_runtime") != 0:
        raise ValueError("wafer-runtime did not exit successfully")
    if metadata.get("wasmtime_version") != "48.0.2":
        raise ValueError("unexpected Wasmtime version")
    if metadata.get("effective_metering_mode") != "fuel-and-epoch":
        raise ValueError("effective metering mode changed")

    source = config["nodes"]["source"]
    sink = config["nodes"]["sink"]
    engine = config["engine"]
    expected_fuel = {"transform": 10_000_000, "filter": 500_000, "router": 500_000}
    if source.get("payload_size") != payload_bytes:
        raise ValueError("payload size does not match the declared condition")
    if source.get("rate") != 1000.0 or source.get("total_messages") != 90_000:
        raise ValueError("source rate or message count changed")
    if source.get("warmup_messages") != 30_000 or sink.get("warmup_secs") != 30:
        raise ValueError("warmup controls changed")
    if engine.get("fuel") != expected_fuel:
        raise ValueError("fuel controls changed")
    if engine.get("default_queue_capacity", 1024) != 1024:
        raise ValueError("queue capacity changed")
    if engine.get("epoch_deadline") != 100 or engine.get("epoch_tick_ms") != 10:
        raise ValueError("epoch controls changed")

    if summary.get("total_count") != 60_000:
        raise ValueError("latency population must contain exactly 60,000 values")
    for field in ("p50_ns", "p95_ns", "p99_ns"):
        value = summary.get(field)
        if not isinstance(value, int) or value < 0:
            raise ValueError(f"invalid latency summary field: {field}")

    throughput = _csv_rows(leaf / "throughput.csv")
    if sum(int(row["msg_count"]) for row in throughput) != 60_000:
        raise ValueError("throughput.csv must total 60,000 messages")

    sequence = _csv_rows(leaf / "sequence.csv")
    if len(sequence) != 1:
        raise ValueError("sequence.csv must contain exactly one summary row")
    sequence_row = sequence[0]
    expected_sequence = {
        "total_expected": 60_000,
        "total_received": 60_000,
        "gap_ranges": 0,
        "gap_msgs": 0,
        "duplicates_count": 0,
    }
    if any(int(sequence_row[field]) != expected for field, expected in expected_sequence.items()):
        raise ValueError("sequence.csv is not lossless and duplicate-free")

    started_ns = int(window["started_ns"])
    finished_ns = int(window["finished_ns"])
    duration_ns = finished_ns - started_ns
    if not 59_000_000_000 <= duration_ns <= 61_500_000_000:
        raise ValueError(f"measurement window is not approximately 60 seconds: {duration_ns}")

    clock_start_ns = int(memory_clock["start_unix_epoch_ns"])
    memory = _csv_rows(leaf / "memory.csv")
    in_window = [
        int(row["rss_bytes"])
        for row in memory
        if started_ns <= clock_start_ns + int(row["elapsed_ms"]) * 1_000_000 < finished_ns
        and int(row["rss_bytes"]) > 0
    ]
    if not in_window:
        raise ValueError("memory.csv has no positive in-window RSS sample")

    config_hash = _sha256(leaf / "config.toml")
    if metadata.get("config_sha256") != config_hash or provenance.get("config_sha256") != config_hash:
        raise ValueError("config provenance differs from config.toml")
    if provenance.get("wafer_runtime_sha256") != metadata.get("wafer_runtime_sha256"):
        raise ValueError("runtime provenance and metadata hashes differ")
    if provenance.get("wafer_plugin_hashes") != metadata.get("wafer_plugin_hashes"):
        raise ValueError("plugin provenance and metadata hashes differ")

    stdout = (leaf / "stdout.log").read_text(errors="replace")
    if re.search(r"thread .* panicked|panicked at|SKIP:|Skipping:|exited before", stdout, re.I):
        raise ValueError("stdout.log contains a panic, skip, or early-exit marker")

    return {
        "measurement_duration_ns": duration_ns,
        "measured_messages": int(sequence_row["total_received"]),
        "peak_rss_bytes": max(in_window),
    }


def _controlled_factors(payload_bytes: int) -> dict[str, Any]:
    return {
        "payload_bytes": payload_bytes,
        "payload_pattern": "repeated-byte-0x42",
        "queue_capacity": 1024,
        "tokio_worker_threads": 4,
        "rate_messages_per_second": 1000,
        "warmup_messages": 30_000,
        "warmup_seconds": 30,
        "measured_messages": 60_000,
        "measurement_seconds": 60,
        "outer_duration_seconds": 120,
        "release": True,
        "locked": True,
        "fuel": {"transform": 10_000_000, "filter": 500_000, "router": 500_000},
        "epoch_deadline_ticks": 100,
        "epoch_tick_ms": 10,
    }


def _worktree_state(worktree: Path) -> dict[str, Any]:
    sha = _command("git", "rev-parse", "HEAD", cwd=worktree)
    status = _command("git", "status", "--porcelain", cwd=worktree)
    if not re.fullmatch(r"[0-9a-f]{40}", sha):
        raise ValueError(f"invalid git SHA for {worktree}")
    if status:
        raise ValueError(f"dirty worktree: {worktree}")
    return {"git_sha": sha, "git_dirty": False}


def _batch_manifest(root: Path, host_metadata: dict[str, Any]) -> dict[str, Any]:
    worktrees = root / "worktrees"
    baseline = worktrees / "baseline"
    candidate = worktrees / "candidate"
    baseline_state = _worktree_state(baseline)
    candidate_state = _worktree_state(candidate)

    config_hashes: dict[str, str] = {}
    for condition in CONDITIONS:
        relative = Path("eval/configs/canonical") / f"e-perf-4-{condition}.toml"
        baseline_hash = _sha256(baseline / relative)
        candidate_hash = _sha256(candidate / relative)
        if baseline_hash != candidate_hash:
            raise ValueError(f"config differs between arms: {condition}")
        config_hashes[condition] = baseline_hash

    baseline_plugin = baseline / "plugins/pass-through/target/wasm32-wasip2/release/wafer_pass_through.wasm"
    candidate_plugin = candidate / "plugins/pass-through/target/wasm32-wasip2/release/wafer_pass_through.wasm"
    baseline_runtime = baseline / "target/release/wafer"
    candidate_runtime = candidate / "target/release/wafer"
    for path in (baseline_plugin, candidate_plugin, baseline_runtime, candidate_runtime):
        if not path.is_file():
            raise ValueError(f"missing prepared artifact: {path}")
    if _sha256(baseline_plugin) != _sha256(candidate_plugin):
        raise ValueError("pass-through component differs between arms")
    if _tree_hash(baseline / "wit") != _tree_hash(candidate / "wit"):
        raise ValueError("WIT tree differs between arms")

    return {
        "schema": BATCH_SCHEMA,
        "generated_at": datetime.now(timezone.utc).isoformat(),
        "host": {
            "hostname": host_metadata.get("hostname"),
            "kernel": host_metadata.get("kernel"),
            "arch": host_metadata.get("arch"),
            "os": host_metadata.get("os"),
        },
        "arms": {"baseline": baseline_state, "candidate": candidate_state},
        "wasmtime_revision": WASMTIME_REVISION,
        "versions": {
            "rustc": _command("rustc", "--version"),
            "cargo": _command("cargo", "--version"),
            "wasm_tools": _command("wasm-tools", "--version"),
            "python": platform.python_version(),
        },
        "wit_tree_sha256": _tree_hash(baseline / "wit"),
        "condition_config_sha256": config_hashes,
        "artifacts": {
            "baseline": {
                "wafer_runtime_sha256": _sha256(baseline_runtime),
                "pass_through_wasm_sha256": _sha256(baseline_plugin),
            },
            "candidate": {
                "wafer_runtime_sha256": _sha256(candidate_runtime),
                "pass_through_wasm_sha256": _sha256(candidate_plugin),
            },
        },
        "controlled_factors": {
            "payload_pattern": "repeated-byte-0x42",
            "queue_capacity": 1024,
            "tokio_worker_threads": 4,
            "rate_messages_per_second": 1000,
            "warmup_messages": 30_000,
            "warmup_seconds": 30,
            "measured_messages": 60_000,
            "measurement_seconds": 60,
            "outer_duration_seconds": 120,
            "release": True,
            "locked": True,
            "fuel": {"transform": 10_000_000, "filter": 500_000, "router": 500_000},
            "epoch_deadline_ticks": 100,
            "epoch_tick_ms": 10,
        },
        "conditions": CONDITIONS,
        "pairs_per_condition": 10,
        "order_rule": "odd-baseline-first-even-candidate-first",
    }


def write_manifest(worktree: Path, arm: str, condition: str, pair: int, output: Path) -> None:
    if arm not in {"baseline", "candidate"}:
        raise ValueError("arm must be baseline or candidate")
    if condition not in CONDITIONS:
        raise ValueError(f"unknown condition: {condition}")
    if not 1 <= pair <= 10:
        raise ValueError("pair index must be in [1, 10]")

    state = _worktree_state(worktree)
    leaf = output.parent
    if leaf.name != f"pair-{pair:02d}" or leaf.parent.name != condition or leaf.parent.parent.name != arm:
        raise ValueError("output path does not match arm/condition/pair")
    root = leaf.parents[2]
    metrics = validate_run_outputs(leaf, state["git_sha"], CONDITIONS[condition])
    metadata = _read_json(leaf / "metadata.json")

    runtime = worktree / "target/release/wafer"
    plugin = worktree / "plugins/pass-through/target/wasm32-wasip2/release/wafer_pass_through.wasm"
    if metadata.get("wafer_runtime_sha256") != _sha256(runtime):
        raise ValueError("metadata runtime hash differs from the prepared binary")
    if metadata.get("wafer_plugin_hashes", {}).get("transform") != _sha256(plugin):
        raise ValueError("metadata plugin hash differs from the prepared component")

    artifact_hashes = {name: _sha256(leaf / name) for name in REQUIRED_FILES}
    artifact_hashes["wafer-runtime"] = _sha256(runtime)
    artifact_hashes["pass-through.wasm"] = _sha256(plugin)
    manifest = {
        "schema": SCHEMA,
        "arm": arm,
        "condition": condition,
        "pair_index": pair,
        **state,
        "wasmtime_revision": WASMTIME_REVISION,
        "controlled_factors": _controlled_factors(CONDITIONS[condition]),
        "observed": metrics,
        "sha256": artifact_hashes,
    }

    batch_path = root / "batch-manifest.json"
    expected_batch = _batch_manifest(root, metadata)
    if batch_path.exists():
        existing = _read_json(batch_path)
        expected_batch["generated_at"] = existing.get("generated_at")
        if existing != expected_batch:
            raise ValueError("existing batch manifest conflicts with the prepared experiment")
    else:
        batch_path.write_text(json.dumps(expected_batch, indent=2, sort_keys=True) + "\n")
    output.write_text(json.dumps(manifest, indent=2, sort_keys=True) + "\n")


def main() -> None:
    parser = argparse.ArgumentParser()
    parser.add_argument("--worktree", required=True, type=Path)
    parser.add_argument("--arm", required=True)
    parser.add_argument("--condition", required=True)
    parser.add_argument("--pair", required=True, type=int)
    parser.add_argument("--output", required=True, type=Path)
    args = parser.parse_args()
    try:
        write_manifest(args.worktree.resolve(), args.arm, args.condition, args.pair, args.output.resolve())
    except (OSError, ValueError, subprocess.CalledProcessError) as error:
        print(f"ERROR: {error}", file=sys.stderr)
        raise SystemExit(1) from error


if __name__ == "__main__":
    main()
