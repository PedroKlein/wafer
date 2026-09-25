#!/usr/bin/env python3

from __future__ import annotations

import argparse
import csv
import hashlib
import json
import math
import re
import statistics
import sys
from datetime import datetime, timezone
from pathlib import Path
from typing import Any

BATCH_SCHEMA = "wafer-p2-async-ab-batch-v1"
RUN_SCHEMA = "wafer-p2-async-ab-run-v1"
DECISION_SCHEMA = "wafer-p2-async-decision-v1"
CONDITIONS = ("120b", "1kb", "100kb")
ARMS = ("baseline", "candidate")
RUN_FILES = (
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


def _csv_rows(path: Path) -> list[dict[str, str]]:
    with path.open(newline="") as stream:
        return list(csv.DictReader(stream))


def _started_at(metadata: dict[str, Any]) -> datetime:
    return datetime.fromisoformat(str(metadata["started_at"]).replace("Z", "+00:00"))


def _run_metrics(leaf: Path, manifest: dict[str, Any]) -> dict[str, Any]:
    hashes = manifest.get("sha256", {})
    for name in RUN_FILES:
        path = leaf / name
        if not path.is_file() or path.stat().st_size == 0:
            raise ValueError(f"missing or empty run artifact: {path}")
        if hashes.get(name) != _sha256(path):
            raise ValueError(f"run artifact hash mismatch: {path}")

    summary = _read_json(leaf / "latency-summary.json")
    sequence_rows = _csv_rows(leaf / "sequence.csv")
    throughput_rows = _csv_rows(leaf / "throughput.csv")
    window = _read_json(leaf / "measurement-window.json")
    memory_clock = _read_json(leaf / "memory-clock.json")
    memory_rows = _csv_rows(leaf / "memory.csv")
    metadata = _read_json(leaf / "metadata.json")

    if summary.get("total_count") != 60_000:
        raise ValueError(f"wrong latency population: {leaf}")
    if len(sequence_rows) != 1:
        raise ValueError(f"wrong sequence row count: {leaf}")
    sequence = sequence_rows[0]
    expected_sequence = (60_000, 60_000, 0, 0, 0)
    actual_sequence = tuple(
        int(sequence[field])
        for field in (
            "total_expected",
            "total_received",
            "gap_ranges",
            "gap_msgs",
            "duplicates_count",
        )
    )
    if actual_sequence != expected_sequence:
        raise ValueError(f"sequence integrity failed: {leaf}")
    if len(throughput_rows) != 60:
        raise ValueError(f"wrong throughput row count: {leaf}")
    measured_messages = sum(int(row["msg_count"]) for row in throughput_rows)
    if measured_messages != 60_000:
        raise ValueError(f"wrong throughput population: {leaf}")

    started_ns = int(window["started_ns"])
    finished_ns = int(window["finished_ns"])
    duration_seconds = (finished_ns - started_ns) / 1_000_000_000
    if not 59.0 <= duration_seconds <= 61.5:
        raise ValueError(f"invalid measurement duration: {leaf}")
    memory_start_ns = int(memory_clock["start_unix_epoch_ns"])
    rss_values = [
        int(row["rss_bytes"])
        for row in memory_rows
        if started_ns <= memory_start_ns + int(row["elapsed_ms"]) * 1_000_000 < finished_ns
        and int(row["rss_bytes"]) > 0
    ]
    if not rss_values:
        raise ValueError(f"no positive in-window RSS sample: {leaf}")

    return {
        "started_at": metadata["started_at"],
        "throughput_messages_per_second": measured_messages / duration_seconds,
        "p50_ns": int(summary["p50_ns"]),
        "p95_ns": int(summary["p95_ns"]),
        "p99_ns": int(summary["p99_ns"]),
        "peak_rss_bytes": max(rss_values),
        "measurement_duration_seconds": duration_seconds,
        "measured_messages": measured_messages,
    }


def _percent_delta(candidate: float, baseline: float) -> float:
    if baseline <= 0 or not math.isfinite(baseline) or not math.isfinite(candidate):
        raise ValueError("metric values must be finite and baseline must be positive")
    return 100.0 * (candidate - baseline) / baseline


def _strict_simplicity(candidate: Path) -> dict[str, Any]:
    source_root = candidate / "crates/wafer-core/src"
    source = "\n".join(path.read_text() for path in sorted(source_root.rglob("*.rs")))
    node_source = "\n".join(
        path.read_text() for path in sorted((source_root / "node").rglob("*.rs"))
    )
    counts = {
        "blocking_bridge_sites": len(re.findall(r"spawn_blocking|block_in_place", source)),
        "sync_linker_sites": source.count("add_to_linker_sync"),
        "sync_typed_instantiation_sites": len(re.findall(r"\.instantiate\(", source)),
        "node_store_mutexes": len(
            re.findall(r"(?:Mutex|RwLock).*Store<WaferState>|Store<WaferState>.*(?:Mutex|RwLock)", node_source)
        ),
        "async_wasi_linkers": source.count("p2::add_to_linker_async"),
        "async_worlds": source.count("require_store_data_send: true"),
        "production_p2_execution_paths": 1,
    }
    passed = counts == {
        "blocking_bridge_sites": 0,
        "sync_linker_sites": 0,
        "sync_typed_instantiation_sites": 0,
        "node_store_mutexes": 0,
        "async_wasi_linkers": 1,
        "async_worlds": 4,
        "production_p2_execution_paths": 1,
    }
    return {"status": "pass" if passed else "fail", **counts}


def _correctness(root: Path, candidate_sha: str) -> dict[str, Any]:
    receipt = root.parent / "p2-correctness-receipt.md"
    if not receipt.is_file():
        return {"status": "fail", "reason": "missing P1-T5 correctness receipt"}
    text = receipt.read_text()
    passed = candidate_sha in text and "- Result: **PASS**" in text
    return {
        "status": "pass" if passed else "fail",
        "receipt": str(receipt),
        "sha256": _sha256(receipt),
    }


def _validate_run_manifest(
    manifest: dict[str, Any], batch: dict[str, Any], arm: str, condition: str, pair: int
) -> None:
    expected_sha = batch["arms"][arm]["git_sha"]
    if (
        manifest.get("schema") != RUN_SCHEMA
        or manifest.get("arm") != arm
        or manifest.get("condition") != condition
        or manifest.get("pair_index") != pair
        or manifest.get("git_sha") != expected_sha
        or manifest.get("git_dirty") is not False
        or manifest.get("wasmtime_revision") != batch.get("wasmtime_revision")
    ):
        raise ValueError(f"invalid run manifest identity: {arm}/{condition}/pair-{pair:02d}")
    factors = manifest.get("controlled_factors", {})
    expected = batch["controlled_factors"] | {"payload_bytes": batch["conditions"][condition]}
    if factors != expected:
        raise ValueError(f"controlled factors differ: {arm}/{condition}/pair-{pair:02d}")
    hashes = manifest.get("sha256", {})
    if hashes.get("wafer-runtime") != batch["artifacts"][arm]["wafer_runtime_sha256"]:
        raise ValueError(f"runtime hash differs: {arm}/{condition}/pair-{pair:02d}")
    if hashes.get("pass-through.wasm") != batch["artifacts"][arm]["pass_through_wasm_sha256"]:
        raise ValueError(f"plugin hash differs: {arm}/{condition}/pair-{pair:02d}")


def _analyze(root: Path, minimum_pairs: int) -> dict[str, Any]:
    batch = _read_json(root / "batch-manifest.json")
    if batch.get("schema") != BATCH_SCHEMA:
        raise ValueError("invalid batch manifest schema")
    if batch.get("pairs_per_condition", 0) < minimum_pairs:
        raise ValueError("batch manifest declares fewer than the required pairs")

    runs: dict[str, dict[str, list[dict[str, Any]]]] = {
        condition: {arm: [] for arm in ARMS} for condition in CONDITIONS
    }
    admitted_pairs: list[str] = []
    for condition in CONDITIONS:
        for pair in range(1, minimum_pairs + 1):
            pair_metrics: dict[str, dict[str, Any]] = {}
            pair_manifests: dict[str, dict[str, Any]] = {}
            pair_metadata: dict[str, dict[str, Any]] = {}
            for arm in ARMS:
                leaf = root / arm / condition / f"pair-{pair:02d}"
                manifest = _read_json(leaf / "p2-ab-manifest.json")
                _validate_run_manifest(manifest, batch, arm, condition, pair)
                pair_manifests[arm] = manifest
                pair_metrics[arm] = _run_metrics(leaf, manifest)
                pair_metadata[arm] = _read_json(leaf / "metadata.json")
            if pair_manifests["baseline"]["sha256"]["config.toml"] != pair_manifests["candidate"]["sha256"]["config.toml"]:
                raise ValueError(f"pair config mismatch: {condition}/pair-{pair:02d}")
            first, second = (("baseline", "candidate") if pair % 2 else ("candidate", "baseline"))
            if _started_at(pair_metadata[first]) >= _started_at(pair_metadata[second]):
                raise ValueError(f"arm order violation: {condition}/pair-{pair:02d}")
            for arm in ARMS:
                runs[condition][arm].append(pair_metrics[arm])
            admitted_pairs.append(f"{condition}/pair-{pair:02d}")

    conditions: dict[str, Any] = {}
    performance_pass = True
    for condition in CONDITIONS:
        baseline = runs[condition]["baseline"]
        candidate = runs[condition]["candidate"]
        baseline_summary = {
            "throughput_median_messages_per_second": statistics.median(
                row["throughput_messages_per_second"] for row in baseline
            ),
            "p50_median_ns": statistics.median(row["p50_ns"] for row in baseline),
            "p95_median_ns": statistics.median(row["p95_ns"] for row in baseline),
            "p99_median_ns": statistics.median(row["p99_ns"] for row in baseline),
            "peak_rss_bytes": max(row["peak_rss_bytes"] for row in baseline),
        }
        candidate_summary = {
            "throughput_median_messages_per_second": statistics.median(
                row["throughput_messages_per_second"] for row in candidate
            ),
            "p50_median_ns": statistics.median(row["p50_ns"] for row in candidate),
            "p95_median_ns": statistics.median(row["p95_ns"] for row in candidate),
            "p99_median_ns": statistics.median(row["p99_ns"] for row in candidate),
            "peak_rss_bytes": max(row["peak_rss_bytes"] for row in candidate),
        }
        throughput_delta = _percent_delta(
            candidate_summary["throughput_median_messages_per_second"],
            baseline_summary["throughput_median_messages_per_second"],
        )
        p50_delta = _percent_delta(
            candidate_summary["p50_median_ns"], baseline_summary["p50_median_ns"]
        )
        p95_delta = _percent_delta(
            candidate_summary["p95_median_ns"], baseline_summary["p95_median_ns"]
        )
        p99_delta = _percent_delta(
            candidate_summary["p99_median_ns"], baseline_summary["p99_median_ns"]
        )
        rss_delta = candidate_summary["peak_rss_bytes"] - baseline_summary["peak_rss_bytes"]
        rss_allowance = max(0.05 * baseline_summary["peak_rss_bytes"], 2 * 1024 * 1024)
        gates = {
            "throughput": throughput_delta >= -5.0,
            "p95_latency": p95_delta <= 5.0,
            "peak_rss": rss_delta <= rss_allowance,
        }
        performance_pass &= all(gates.values())
        pair_deltas = [
            {
                "pair_index": index,
                "throughput_pct": _percent_delta(
                    candidate_run["throughput_messages_per_second"],
                    baseline_run["throughput_messages_per_second"],
                ),
                "p50_latency_pct": _percent_delta(candidate_run["p50_ns"], baseline_run["p50_ns"]),
                "p95_latency_pct": _percent_delta(candidate_run["p95_ns"], baseline_run["p95_ns"]),
                "p99_latency_pct": _percent_delta(candidate_run["p99_ns"], baseline_run["p99_ns"]),
                "rss_bytes": candidate_run["peak_rss_bytes"] - baseline_run["peak_rss_bytes"],
            }
            for index, (baseline_run, candidate_run) in enumerate(
                zip(baseline, candidate, strict=True), start=1
            )
        ]
        conditions[condition] = {
            "baseline_runs": baseline,
            "candidate_runs": candidate,
            "pair_deltas": pair_deltas,
            "baseline": baseline_summary,
            "candidate": candidate_summary,
            "deltas": {
                "throughput_pct": throughput_delta,
                "p50_latency_pct": p50_delta,
                "p95_latency_pct": p95_delta,
                "p99_latency_pct": p99_delta,
                "rss_bytes": rss_delta,
                "rss_allowance_bytes": rss_allowance,
            },
            "gates": gates,
        }

    candidate_sha = batch["arms"]["candidate"]["git_sha"]
    correctness = _correctness(root, candidate_sha)
    simplicity = _strict_simplicity(root / "worktrees/candidate")
    decision = (
        "adopt-async-p2"
        if correctness["status"] == "pass"
        and simplicity["status"] == "pass"
        and performance_pass
        else "retain-sync-p2"
    )
    return {
        "schema": DECISION_SCHEMA,
        "generated_at": datetime.now(timezone.utc).isoformat(),
        "analysis_tool_sha256": _sha256(Path(__file__).resolve()),
        "baseline_sha": batch["arms"]["baseline"]["git_sha"],
        "candidate_sha": candidate_sha,
        "wasmtime_revision": batch["wasmtime_revision"],
        "minimum_pairs": minimum_pairs,
        "admitted_pairs": admitted_pairs,
        "correctness": correctness,
        "strict_simplicity": simplicity,
        "performance": {"status": "pass" if performance_pass else "fail"},
        "conditions": conditions,
        "decision": decision,
    }


def analyze(root: Path, minimum_pairs: int, output: Path) -> dict[str, Any]:
    try:
        result = _analyze(root, minimum_pairs)
    except (KeyError, OSError, TypeError, ValueError, json.JSONDecodeError) as error:
        result = {
            "schema": DECISION_SCHEMA,
            "generated_at": datetime.now(timezone.utc).isoformat(),
            "decision": "retain-sync-p2",
            "analysis_status": "failed",
            "errors": [str(error)],
        }
        output.write_text(json.dumps(result, indent=2, sort_keys=True) + "\n")
        raise
    output.write_text(json.dumps(result, indent=2, sort_keys=True) + "\n")
    return result


def main() -> None:
    parser = argparse.ArgumentParser()
    parser.add_argument("--root", required=True, type=Path)
    parser.add_argument("--minimum-pairs", required=True, type=int)
    parser.add_argument("--output", required=True, type=Path)
    args = parser.parse_args()
    try:
        result = analyze(args.root.resolve(), args.minimum_pairs, args.output.resolve())
    except (KeyError, OSError, TypeError, ValueError, json.JSONDecodeError) as error:
        print(f"ERROR: {error}", file=sys.stderr)
        raise SystemExit(1) from error
    print(json.dumps(result, indent=2, sort_keys=True))


if __name__ == "__main__":
    main()
