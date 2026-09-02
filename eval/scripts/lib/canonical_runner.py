from __future__ import annotations

import argparse
import csv
import datetime as dt
import hashlib
import json
import os
import random
import re
import shutil
import signal
import statistics
import subprocess
import sys
import threading
import time
import tomllib
import urllib.error
import urllib.request
from collections import Counter
from dataclasses import dataclass
from pathlib import Path

from write_metadata import merge_metadata


@dataclass(frozen=True)
class Condition:
    name: str
    config: str
    loadgen_profile: str | None = None
    total_messages: int | None = None
    shared_from: str | None = None
    startup_mode: str | None = None
    system: str = "wafer"
    events_per_run: int | None = None
    offered_rate_msg_s: int | None = None
    exclusive_sut: bool = False


@dataclass(frozen=True)
class RunItem:
    experiment: str
    condition: str
    run_index: int
    config: str
    warmup_secs: int
    measurement_secs: int
    runtime_cpus: str = "1-3"
    support_cpus: str = "0"
    loadgen_profile: str | None = None
    total_messages: int | None = None
    shared_from: str | None = None
    startup_mode: str | None = None
    system: str = "wafer"
    events_per_run: int | None = None
    offered_rate_msg_s: int | None = None
    exclusive_sut: bool = False

    @property
    def result_key(self) -> str:
        return f"{self.experiment}/{self.condition}/run-{self.run_index:02d}"


@dataclass(frozen=True)
class AttemptSelection:
    path: Path
    skip: bool


@dataclass(frozen=True)
class ValidationGate:
    passed: bool
    failed_runs: list[int]
    observed_runs: int


RATE_SWEEP_SYSTEMS = ("mqtt-loopback", "native", "wafer", "ekuiper")
RATE_SWEEP_RATES = (500, 1_000, 2_000, 4_000, 8_000, 16_000)
E_VAL_1_MIN_P99_NS = 45_000_000
# HdrHistogram reports the upper bound of the 3-significant-digit bucket containing 55 ms.
E_VAL_1_MAX_P99_NS = 55_017_471
RATE_SWEEP_BASELINE = 1_000
RATE_SWEEP_P99_MULTIPLIER = 2.0
RATE_SWEEP_MAX_LOSS_PERCENT = 1.0
RATE_SWEEP_PROFILE = "eval/loadgen/canonical-rate-sweep.toml"
STARTUP_PHASES = (
    "process_config",
    "component_load_compile",
    "instantiation",
    "pipeline_setup",
    "first_process",
)
STARTUP_HARNESS_OVERHEAD_TOLERANCE_NS = 5_000_000
HOTSWAP_SHARED_EXPERIMENTS = {"e-swap-1", "e-swap-2", "e-swap-4", "e-swap-6"}
HOTSWAP_PHASE_FIELDS = (
    "compile_ns",
    "instantiate_ns",
    "signal_ns",
    "ack_ns",
    "convergence_ns",
)
FOCUSED_MATRIX_SHA_ENV = "WAFER_FOCUSED_MATRIX_SHA256"


def derive_hotswap_evidence(
    requests: list[dict],
    sink_timeline: dict,
    *,
    experiment: str,
    condition: str,
    source_leaf: str,
) -> dict:
    transitions = sink_timeline.get("transitions")
    if not isinstance(transitions, list):
        raise ValueError("swap_timeline.json must contain a transitions array")
    if len(requests) != len(transitions):
        raise ValueError(
            f"hot-swap request/transition count mismatch: {len(requests)} != {len(transitions)}"
        )

    events = []
    for index, (request, transition) in enumerate(zip(requests, transitions, strict=True)):
        timeline = request.get("body", {}).get("timeline", {})
        for field in HOTSWAP_PHASE_FIELDS:
            value = timeline.get(field)
            if type(value) is not int or value < 0:
                raise ValueError(f"hot-swap event {index} requires non-negative integer {field}")
        http_total_ns = request.get("request_duration_ns")
        output_gap_ns = transition.get("pause_ns")
        if type(http_total_ns) is not int or http_total_ns < 0:
            raise ValueError(f"hot-swap event {index} requires non-negative integer request_duration_ns")
        if type(output_gap_ns) is not int or output_gap_ns < 0:
            raise ValueError(f"hot-swap event {index} requires non-negative integer pause_ns")
        events.append(
            {
                "event_index": int(request.get("event_index", index)),
                **{field: timeline[field] for field in HOTSWAP_PHASE_FIELDS},
                "http_total_ns": http_total_ns,
                "http_total_clock": request.get(
                    "request_duration_clock", "wall-clock-difference-legacy"
                ),
                "sink_observed_output_gap_ns": output_gap_ns,
            }
        )

    return {
        "schema_version": 1,
        "duration_unit": "ns",
        "experiment": experiment,
        "condition": condition,
        "measurement_source_leaf": source_leaf,
        "shared_from": None,
        "sample_count": len(events),
        "events": events,
        "interpretation": (
            "Internal swap phases, HTTP request duration, and sink-observed output gaps are "
            "separate measurements. A smaller sink-observed output gap does not imply a faster "
            "internal swap because queued output can mask internal disruption."
        ),
    }


def analyze_backpressure(
    samples: list[dict],
    *,
    offered_messages: int,
    offered_duration_ns: int,
    occupancy_threshold: float,
    recovery_threshold: float,
) -> dict:
    if not samples:
        raise ValueError("queue-depth samples are empty")
    if offered_duration_ns <= 0:
        raise ValueError("offered duration must be positive")
    if not 0 <= recovery_threshold < occupancy_threshold <= 1:
        raise ValueError("queue thresholds must satisfy 0 <= recovery < occupancy <= 1")

    by_queue: dict[str, list[dict]] = {}
    for sample in samples:
        capacity = int(sample["capacity"])
        depth = int(sample["depth"])
        if capacity <= 0 or depth < 0 or depth > capacity:
            raise ValueError("queue depth must be within its positive capacity")
        by_queue.setdefault(str(sample["queue"]), []).append(sample)

    selected_queue, selected = max(
        by_queue.items(),
        key=lambda item: max(int(row["depth"]) / int(row["capacity"]) for row in item[1]),
    )
    selected.sort(key=lambda row: int(row["elapsed_ns"]))
    peak_index = max(
        range(len(selected)),
        key=lambda index: int(selected[index]["depth"]) / int(selected[index]["capacity"]),
    )
    peak = selected[peak_index]
    peak_occupancy = int(peak["depth"]) / int(peak["capacity"])
    threshold_crossed = peak_occupancy >= occupancy_threshold

    recovery = next(
        (
            row
            for row in selected[peak_index + 1 :]
            if int(row["depth"]) / int(row["capacity"]) <= recovery_threshold
        ),
        None,
    )
    recovered = threshold_crossed and recovery is not None
    elapsed_ns = int(selected[-1]["elapsed_ns"]) - int(selected[0]["elapsed_ns"])
    if elapsed_ns <= 0:
        raise ValueError("queue-depth samples must span positive elapsed time")

    accepted = max(int(row["accepted"]) for row in selected)
    processed = max(int(row["processed"]) for row in selected)
    drained_rate = None
    if recovered:
        drain_duration_ns = int(recovery["elapsed_ns"]) - int(peak["elapsed_ns"])
        drained_messages = int(recovery["processed"]) - int(peak["processed"])
        if drain_duration_ns > 0 and drained_messages >= 0:
            drained_rate = drained_messages * 1_000_000_000 / drain_duration_ns

    return {
        "schema_version": 1,
        "clock": "monotonic",
        "queue": selected_queue,
        "sample_count": len(selected),
        "capacity_messages": int(peak["capacity"]),
        "peak_depth_messages": int(peak["depth"]),
        "peak_occupancy": peak_occupancy,
        "occupancy_threshold": occupancy_threshold,
        "recovery_threshold": recovery_threshold,
        "threshold_crossed": threshold_crossed,
        "recovered": recovered,
        "classification": (
            "saturated-and-drained"
            if recovered
            else "saturated-not-drained"
            if threshold_crossed
            else "not-saturated"
        ),
        "counts": {
            "offered": offered_messages,
            "accepted": accepted,
            "processed": processed,
            "outstanding": accepted - processed,
        },
        "rates_msg_s": {
            "offered": offered_messages * 1_000_000_000 / offered_duration_ns,
            "accepted": accepted * 1_000_000_000 / elapsed_ns,
            "processed": processed * 1_000_000_000 / elapsed_ns,
            "drained": drained_rate,
        },
    }


def read_queue_depth(path: Path) -> list[dict]:
    with path.open(newline="") as handle:
        return [dict(row) for row in csv.DictReader(handle)]


def validate_backpressure_result(result: dict) -> None:
    if result.get("classification") != "saturated-and-drained":
        raise ValueError("backpressure run did not cross and recover below queue thresholds")
    if set(result.get("rates_msg_s", {})) != {"offered", "accepted", "processed", "drained"}:
        raise ValueError("backpressure rates must include offered, accepted, processed, and drained")
    if result["rates_msg_s"]["drained"] is None:
        raise ValueError("backpressure run has no measurable drain rate")
    if not result.get("sequence", {}).get("lossless"):
        raise ValueError("backpressure slow policy lost or duplicated messages")
    if not result.get("memory", {}).get("within_limit"):
        raise ValueError("backpressure run exceeded the frozen RSS bound")


def _validate_rate_sweep_definition(root: Path, definition: dict) -> None:
    profile = tomllib.loads((root / RATE_SWEEP_PROFILE).read_text())["sweep"]
    expected = {
        "systems": list(RATE_SWEEP_SYSTEMS),
        "rate_points_msg_s": list(RATE_SWEEP_RATES),
        "repetitions": profile["repetitions"],
        "thesis_evidence": profile["thesis_evidence"],
    }
    for field, value in expected.items():
        if definition.get(field) != value:
            raise ValueError(f"e-perf-10 {field} differs from canonical rate-sweep profile")
    criteria = definition.get("sustainable_throughput", {})
    for field, value in {
        "baseline_rate_msg_s": profile["baseline_rate_msg_s"],
        "p99_multiplier_limit": profile["p99_multiplier_limit"],
        "max_loss_percent": profile["max_loss_percent"],
        "p99_aggregation": profile["p99_aggregation"],
        "loss_aggregation": profile["loss_aggregation"],
    }.items():
        if criteria.get(field) != value:
            raise ValueError(f"e-perf-10 {field} differs from canonical rate-sweep profile")


def _rate_sweep_config(system: str) -> str:
    if system == "wafer":
        return "eval/configs/pipeline-a-wafer.toml"
    if system == "native":
        return "eval/configs/pipeline-a-native.toml"
    if system == "ekuiper":
        return "eval/configs/canonical/e-perf-1-ekuiper.toml"
    return RATE_SWEEP_PROFILE


CONDITIONS: dict[str, tuple[Condition, ...]] = {
    "e-perf-1": tuple(
        Condition(
            system,
            "eval/configs/pipeline-a-wafer.toml"
            if system == "wafer"
            else "eval/configs/pipeline-a-native.toml"
            if system == "native"
            else "eval/configs/canonical/e-perf-1-ekuiper.toml",
            "eval/loadgen/telemetry-120b.toml",
            60_000,
            system=system,
        )
        for system in ("wafer", "native", "ekuiper")
    ),
    "e-perf-10": tuple(
        Condition(
            f"{system}/rate-{rate:05d}",
            _rate_sweep_config(system),
            RATE_SWEEP_PROFILE,
            system=system,
            offered_rate_msg_s=rate,
            exclusive_sut=True,
        )
        for system in RATE_SWEEP_SYSTEMS
        for rate in RATE_SWEEP_RATES
    ),
    "e-perf-2": tuple(
        Condition(
            system,
            "eval/configs/pipeline-a-wafer.toml"
            if system == "wafer"
            else "eval/configs/pipeline-a-native.toml"
            if system == "native"
            else "eval/configs/canonical/e-perf-1-ekuiper.toml",
            "eval/loadgen/telemetry-120b.toml",
            60_000,
            shared_from="e-perf-1",
            system=system,
        )
        for system in ("wafer", "native", "ekuiper")
    ),
    "e-val-1": (
        Condition("delay-50ms", "eval/configs/canonical/e-val-1.toml"),
    ),
    "e-perf-3": tuple(
        Condition(
            f"depth-{depth}",
            f"eval/configs/e-perf-3/pipeline-mqtt-depth-{depth}.toml",
            "eval/loadgen/telemetry-120b.toml",
            60_000,
        )
        for depth in (1, 3, 5, 10)
    ),
    "e-perf-4": tuple(
        Condition(
            size,
            f"eval/configs/canonical/e-perf-4-{size}.toml",
        )
        for size in ("120b", "1kb", "10kb", "100kb")
    ),
    "e-perf-6": tuple(
        Condition(
            f"depth-{depth}",
            f"eval/configs/pipeline-depth-{depth}.toml",
        )
        for depth in (1, 3, 5, 10)
    ),
    "e-perf-8": tuple(
        Condition(
            f"depth-{depth}",
            f"eval/configs/pipeline-depth-{depth}.toml",
            shared_from="e-perf-6",
        )
        for depth in (1, 3, 5, 10)
    ),
    "e-perf-7": tuple(
        Condition(
            mode,
            f"eval/configs/pipeline-c-{mode}.toml",
        )
        for mode in ("neither", "fuel-only", "epoch-only", "passthrough")
    ),
    "e-perf-9": tuple(
        Condition(
            f"{tier}-{cache}",
            f"eval/configs/e-perf-9/pipeline-tier-{tier}.toml",
            startup_mode=cache,
        )
        for tier in ("small", "medium", "large")
        for cache in ("cold", "warm")
    ),
    "e-perf-5": (
        Condition("wafer", "eval/configs/pipeline-c-passthrough.toml", system="wafer"),
        Condition("native", "eval/configs/pipeline-d-native.toml", system="native"),
    ),
    "e-swap-3": tuple(
        Condition(
            strategy,
            "eval/configs/e-swap/pipeline-swap3-mqtt.toml"
            if strategy != "ekuiper-restart"
            else "eval/configs/canonical/e-perf-1-ekuiper.toml",
            "eval/loadgen/canonical-hotswap.toml"
            if strategy == "wafer-hotswap"
            else "eval/loadgen/telemetry-120b.toml",
            120_000,
            system="ekuiper" if strategy == "ekuiper-restart" else "wafer",
        )
        for strategy in ("wafer-hotswap", "wafer-restart", "ekuiper-restart")
    ),
    "e-iso-1": (Condition("buffer-overflow", "eval/configs/e-iso-1/pipeline.toml"),),
    "e-iso-2": (Condition("cross-read", "eval/configs/e-iso-2/pipeline.toml"),),
    "e-iso-3": (Condition("fs-access", "eval/configs/e-iso-3/pipeline.toml"),),
    "e-iso-4": (Condition("infinite-loop", "eval/configs/e-iso-4/pipeline.toml"),),
    "e-iso-5": (Condition("memory-exhaust", "eval/configs/e-iso-5/pipeline.toml"),),
    "e-iso-6": (Condition("panic", "eval/configs/e-iso-6/pipeline.toml"),),
    "e-iso-7": (
        Condition("control", "eval/configs/e-iso-7/pipeline-control.toml"),
        Condition("panic-attack", "eval/configs/e-iso-7/pipeline.toml"),
        Condition("epoch-loop-attack", "eval/configs/e-iso-7/pipeline-epoch-attack.toml"),
    ),
    "e-iso-8": (Condition("panic-recovery", "eval/configs/e-iso-8/pipeline.toml"),),
    "e-swap-1": (
        Condition("steady", "eval/configs/e-swap/pipeline-hotswap.toml", events_per_run=50),
    ),
    "e-swap-2": (
        Condition(
            "steady",
            "eval/configs/e-swap/pipeline-hotswap.toml",
            shared_from="e-swap-1",
            events_per_run=50,
        ),
    ),
    "e-swap-4": (
        Condition("burst-2x", "eval/configs/e-swap/pipeline-hotswap-burst.toml", events_per_run=50),
    ),
    "e-swap-5": (
        Condition(
            "process-trap-rollback",
            "eval/configs/e-swap/pipeline-hotswap-rollback.toml",
            events_per_run=50,
        ),
    ),
    "e-swap-6": (
        Condition(
            "steady",
            "eval/configs/e-swap/pipeline-hotswap.toml",
            shared_from="e-swap-1",
            events_per_run=50,
        ),
    ),
    "e-backpressure": (
        Condition(
            "saturated-slow-consumer",
            "eval/configs/e-backpressure/pipeline-saturated.toml",
            total_messages=1_000,
        ),
    ),
}


EXPERIMENT_ORDER = (
    "e-val-1",
    "e-perf-1",
    "e-perf-2",
    "e-perf-3",
    "e-perf-4",
    "e-perf-5",
    "e-perf-6",
    "e-perf-8",
    "e-perf-7",
    "e-perf-9",
    "e-perf-10",
    "e-backpressure",
    "e-iso-1",
    "e-iso-2",
    "e-iso-3",
    "e-iso-4",
    "e-iso-5",
    "e-iso-6",
    "e-iso-7",
    "e-iso-8",
    "e-swap-1",
    "e-swap-2",
    "e-swap-3",
    "e-swap-4",
    "e-swap-5",
    "e-swap-6",
)


def build_schedule(experiments: set[str], seed: int) -> list[RunItem]:
    unknown = experiments - CONDITIONS.keys()
    if unknown:
        raise ValueError(f"unsupported experiments: {', '.join(sorted(unknown))}")

    matrix_path = Path(__file__).resolve().parents[2] / "canonical-matrix.json"
    matrix = json.loads(matrix_path.read_text())["experiments"]
    if "e-perf-10" in experiments:
        _validate_rate_sweep_definition(matrix_path.parents[1], matrix["e-perf-10"])
    schedule: list[RunItem] = []

    for experiment in (item for item in EXPERIMENT_ORDER if item in experiments):
        definition = matrix[experiment]
        conditions = CONDITIONS[experiment]
        for run_index in range(1, definition["repetitions"] + 1):
            if experiment == "e-perf-9":
                ordered = list(conditions)
            elif experiment == "e-perf-10":
                by_pair = {
                    (condition.system, condition.offered_rate_msg_s): condition
                    for condition in conditions
                }
                rates = list(RATE_SWEEP_RATES)
                random.Random(f"{seed}:{experiment}:{run_index}:rates").shuffle(rates)
                ordered = []
                for rate in rates:
                    systems = list(RATE_SWEEP_SYSTEMS)
                    random.Random(f"{seed}:{experiment}:{rate}:systems").shuffle(systems)
                    offset = (run_index - 1) % len(systems)
                    systems = systems[offset:] + systems[:offset]
                    ordered.extend(by_pair[(system, rate)] for system in systems)
            else:
                ordered = list(conditions)
                random.Random(f"{seed}:{experiment}:{run_index}").shuffle(ordered)
            for condition in ordered:
                total_messages = condition.total_messages
                if condition.offered_rate_msg_s is not None:
                    total_messages = condition.offered_rate_msg_s * definition["measurement_secs"]
                schedule.append(
                    RunItem(
                        experiment=experiment,
                        condition=condition.name,
                        run_index=run_index,
                        config=condition.config,
                        warmup_secs=definition["warmup_secs"],
                        measurement_secs=definition["measurement_secs"],
                        loadgen_profile=condition.loadgen_profile,
                        total_messages=total_messages,
                        shared_from=condition.shared_from,
                        startup_mode=condition.startup_mode,
                        system=condition.system,
                        events_per_run=condition.events_per_run or definition.get("events_per_run"),
                        offered_rate_msg_s=condition.offered_rate_msg_s,
                        exclusive_sut=condition.exclusive_sut,
                    )
                )
    return schedule


def build_focused_schedule(seed: int) -> list[RunItem]:
    matrix_path = Path(__file__).resolve().parents[2] / "canonical-matrix.json"
    focused = json.loads(matrix_path.read_text())["focused_pilot"]
    if seed != focused["seed"]:
        raise ValueError(f"focused pilot seed must be {focused['seed']}")
    selected = focused["experiments"]
    schedule = build_schedule(set(selected), seed)
    return [
        item
        for item in schedule
        if item.condition in selected[item.experiment]["condition_runs"]
        and item.run_index in selected[item.experiment]["condition_runs"][item.condition]
    ]


def select_attempt(condition_dir: Path, run_index: int) -> AttemptSelection:
    attempts = sorted(condition_dir.glob(f"run-{run_index:02d}-attempt-*"))
    for attempt in attempts:
        status_path = attempt / "canonical-status.json"
        try:
            status = json.loads(status_path.read_text())
        except (OSError, ValueError):
            continue
        if status.get("status") == "passed":
            return AttemptSelection(attempt, True)

    next_index = len(attempts) + 1
    path = condition_dir / f"run-{run_index:02d}-attempt-{next_index:02d}"
    return AttemptSelection(path, False)


def evaluate_validation_gate(root: Path, expected_runs: int) -> ValidationGate:
    failed: list[int] = []
    observed = 0
    for run_index in range(1, expected_runs + 1):
        attempts = sorted(root.glob(f"run-{run_index:02d}-attempt-*"))
        passed_attempt = None
        for attempt in attempts:
            try:
                status = json.loads((attempt / "canonical-status.json").read_text())
            except (OSError, ValueError):
                continue
            if status.get("status") == "passed":
                passed_attempt = attempt
                break
        if passed_attempt is None:
            failed.append(run_index)
            continue
        observed += 1
        try:
            summary = json.loads((passed_attempt / "percentiles.json").read_text())
            p99_ns = int(summary["p99_ns"])
            count = int(summary["total_count"])
        except (OSError, ValueError, KeyError, TypeError):
            failed.append(run_index)
            continue
        if count <= 0 or not E_VAL_1_MIN_P99_NS <= p99_ns <= E_VAL_1_MAX_P99_NS:
            failed.append(run_index)

    return ValidationGate(not failed and observed == expected_runs, failed, observed)


def _summarize_branch_artifacts(
    output: Path,
    branch_dir: str,
    offered_messages: int | None,
) -> dict:
    branch = output / branch_dir
    with (branch / "sequence.csv").open(newline="") as stream:
        rows = list(csv.DictReader(stream))
    if len(rows) != 1:
        raise ValueError(f"{branch_dir}/sequence.csv must contain one summary row")
    try:
        sequence = {field: int(rows[0][field]) for field in (
            "total_expected",
            "total_received",
            "gap_msgs",
            "duplicates_count",
        )}
    except (KeyError, TypeError, ValueError) as error:
        raise ValueError(f"{branch_dir}/sequence.csv contains invalid counts") from error

    with (branch / "throughput.csv").open(newline="") as stream:
        try:
            throughput = [
                {
                    "elapsed_seconds": float(row["elapsed_secs"]),
                    "msg_count": int(row["msg_count"]),
                    "bytes": int(row["bytes"]),
                }
                for row in csv.DictReader(stream)
            ]
        except (KeyError, TypeError, ValueError) as error:
            raise ValueError(f"{branch_dir}/throughput.csv contains invalid samples") from error

    try:
        percentiles = json.loads((branch / "percentiles.json").read_text())
        latency = {
            "sample_count": int(percentiles["total_count"]),
            "p50": int(percentiles["p50_ns"]),
            "p95": int(percentiles["p95_ns"]),
            "p99": int(percentiles["p99_ns"]),
            "p999": int(percentiles["p999_ns"]),
        }
    except (KeyError, OSError, TypeError, ValueError) as error:
        raise ValueError(f"{branch_dir}/percentiles.json is invalid") from error

    window_path = branch / "measurement-window.json"
    window = None
    if window_path.is_file():
        try:
            raw_window = json.loads(window_path.read_text())
            window = {
                "started_ns": int(raw_window["started_ns"]),
                "finished_ns": int(raw_window["finished_ns"]),
            }
        except (KeyError, OSError, TypeError, ValueError) as error:
            raise ValueError(f"{branch_dir}/measurement-window.json is invalid") from error
        if window["finished_ns"] <= window["started_ns"]:
            raise ValueError(f"{branch_dir}/measurement-window.json is empty or reversed")

    total_messages = sum(sample["msg_count"] for sample in throughput)
    duration_seconds = (
        (window["finished_ns"] - window["started_ns"]) / 1_000_000_000
        if window
        else 0.0
    )
    offered = offered_messages or sequence["total_expected"]
    return {
        "artifact_dir": branch_dir,
        "offered_messages": offered,
        "expected_next_sequence": sequence["total_expected"],
        "received_messages": sequence["total_received"],
        "lost_messages": max(0, offered - sequence["total_received"]),
        "gap_messages": sequence["gap_msgs"],
        "duplicates": sequence["duplicates_count"],
        "measurement_window": window,
        "throughput": {
            "total_messages": total_messages,
            "mean_messages_per_second": total_messages / duration_seconds if duration_seconds else 0.0,
            "samples": throughput,
        },
        "latency_ns": latency,
    }


def derive_branch_isolation(
    output: Path,
    warmup_secs: int,
    measurement_secs: int,
    offered_messages: int | None = None,
) -> dict:
    return {
        "schema_version": 1,
        "experiment": "e-iso-7",
        "measurement_boundary": {
            "kind": "branch_sink_post_warmup",
            "warmup_secs": warmup_secs,
            "measurement_secs": measurement_secs,
        },
        "units": {
            "counts": "messages",
            "throughput": "messages_per_second",
            "latency": "nanoseconds",
            "timestamps": "unix_epoch_nanoseconds",
        },
        "branches": {
            "branch_a": _summarize_branch_artifacts(output, "branch-a", offered_messages),
            "branch_b": _summarize_branch_artifacts(output, "branch-b", offered_messages),
        },
    }


def compare_branch_a(control_runs: list[dict], attack_runs: list[dict]) -> dict:
    if not control_runs or not attack_runs:
        raise ValueError("branch-A comparison requires control and attack runs")

    def condition_summary(runs: list[dict]) -> dict:
        branches = [run["branches"]["branch_a"] for run in runs]
        return {
            "run_count": len(branches),
            "median_throughput_messages_per_second": statistics.median(
                branch["throughput"]["mean_messages_per_second"] for branch in branches
            ),
            "median_p95_latency_ns": statistics.median(
                branch["latency_ns"]["p95"] for branch in branches
            ),
        }

    control = condition_summary(control_runs)
    attack = condition_summary(attack_runs)
    control_throughput = control["median_throughput_messages_per_second"]
    control_p95 = control["median_p95_latency_ns"]
    return {
        "control": control,
        "attack": attack,
        "branch_a_impact": {
            "throughput_drop_percent": (
                (control_throughput - attack["median_throughput_messages_per_second"])
                / control_throughput
                * 100
                if control_throughput
                else 0.0
            ),
            "p95_latency_increase_percent": (
                (attack["median_p95_latency_ns"] - control_p95) / control_p95 * 100
                if control_p95
                else 0.0
            ),
        },
        "units": {
            "throughput": "messages_per_second",
            "latency": "nanoseconds",
            "impact": "percent",
        },
    }


def compare_branch_conditions(runs: dict[str, list[dict]]) -> dict[str, dict]:
    control = runs.get("control", [])
    attacks = {
        condition: samples
        for condition, samples in runs.items()
        if condition != "control"
    }
    if not attacks:
        raise ValueError("branch-A comparison requires at least one attack condition")
    return {
        condition: compare_branch_a(control, samples)
        for condition, samples in sorted(attacks.items())
    }


def derive_containment(output: Path) -> dict:
    with (output / "per_node_metrics.csv").open(newline="") as stream:
        rows = [
            row
            for row in csv.DictReader(stream)
            if row.get("node_id") and not row["node_id"].startswith("#")
        ]
    if not rows:
        raise ValueError("per_node_metrics.csv contains no runtime metric rows")
    try:
        traps_total = sum(int(row["traps_total"]) for row in rows)
        healthy_messages = max(
            (int(row["messages_out"]) for row in rows if row["node_id"] not in {"attack", "branch_b"}),
            default=0,
        )
    except (KeyError, TypeError, ValueError) as error:
        raise ValueError("per_node_metrics.csv contains invalid runtime metrics") from error
    log = (output / "stdout.log").read_text(errors="replace")
    runtime_panic = bool(re.search(r"thread .* panicked|panicked at", log))
    return {
        "contained": traps_total > 0 and not runtime_panic,
        "traps_total": traps_total,
        "healthy_messages_out": healthy_messages,
        "runtime_panic": runtime_panic,
        "nodes": rows,
    }


def summarize_recovery(path: Path) -> dict:
    with path.open(newline="") as stream:
        samples = [int(row["duration_ns"]) for row in csv.DictReader(stream)]
    if not samples:
        raise ValueError("recovery.csv contains no samples")
    ordered = sorted(samples)

    def percentile(fraction: float) -> int:
        index = max(0, min(len(ordered) - 1, int(len(ordered) * fraction + 0.999999) - 1))
        return ordered[index]

    return {
        "sample_count": len(ordered),
        "min_ns": ordered[0],
        "p50_ns": percentile(0.50),
        "p95_ns": percentile(0.95),
        "p99_ns": percentile(0.99),
        "max_ns": ordered[-1],
    }


def _read_integer_csv(path: Path, fields: tuple[str, ...]) -> list[dict[str, int]]:
    with path.open(newline="") as stream:
        reader = csv.DictReader(stream)
        missing = [field for field in fields if field not in (reader.fieldnames or [])]
        if missing:
            raise ValueError(f"{path} missing columns: {', '.join(missing)}")
        try:
            return [{field: int(row[field]) for field in fields} for row in reader]
        except (TypeError, ValueError) as error:
            raise ValueError(f"{path} contains invalid integers") from error


def analyze_rate_sweep_traces(
    published_path: Path,
    received_path: Path,
    subscriber_metadata_path: Path,
) -> dict:
    published = _read_integer_csv(published_path, ("seq", "ts_ns"))
    received = _read_integer_csv(
        received_path,
        ("seq", "payload_ts_ns", "receive_ns", "latency_ns"),
    )
    published_by_seq = {row["seq"]: row["ts_ns"] for row in published}
    if len(published_by_seq) != len(published):
        raise ValueError("publisher trace contains duplicate sequences")

    received_counts = Counter(row["seq"] for row in received)
    unexpected = sorted(set(received_counts) - set(published_by_seq))
    if unexpected:
        raise ValueError(f"received unexpected sequences: {unexpected}")
    for row in received:
        expected_ts = published_by_seq[row["seq"]]
        if row["payload_ts_ns"] != expected_ts:
            raise ValueError(f"timestamp changed for sequence {row['seq']}")
        if row["receive_ns"] < row["payload_ts_ns"]:
            raise ValueError(f"negative latency for sequence {row['seq']}")
        if row["latency_ns"] != row["receive_ns"] - row["payload_ts_ns"]:
            raise ValueError(f"latency mismatch for sequence {row['seq']}")

    metadata = json.loads(subscriber_metadata_path.read_text())
    if metadata.get("parse_errors") != 0 or metadata.get("negative_latency_count") != 0:
        raise ValueError("subscriber reported parse errors or negative latency")
    if metadata.get("total_messages") != len(received) or metadata.get("total_recorded") != len(received):
        raise ValueError("subscriber metadata count differs from received trace")
    sequence = metadata.get("sequence", {})
    duplicates = sum(count - 1 for count in received_counts.values() if count > 1)
    if sequence.get("total_received") != len(received) or sequence.get("total_duplicates") != duplicates:
        raise ValueError("subscriber sequence metadata differs from received trace")
    unique_received = len(received_counts)
    offered = len(published_by_seq)
    return {
        "messages": {
            "offered": offered,
            "received": unique_received,
            "lost": max(0, offered - unique_received),
            "duplicates": duplicates,
        },
        "latency_ns": {
            "p50": int(metadata["latency_p50_ns"]),
            "p95": int(metadata["latency_p95_ns"]),
            "p99": int(metadata["latency_p99_ns"]),
        },
    }


def summarize_process_resources(path: Path, clock_ticks: int | None = None) -> dict:
    rows = _read_integer_csv(
        path,
        ("timestamp_ns", "cpu_time_ticks", "rss_bytes", "process_count"),
    )
    if not rows:
        raise ValueError("resource-usage.csv contains no samples")
    scope = "sut" if any(row["process_count"] > 0 for row in rows) else "no-sut"
    if scope == "sut" and any(row["process_count"] == 0 for row in rows):
        raise ValueError("SUT disappeared during resource sampling")
    ticks_per_second = clock_ticks or int(os.sysconf("SC_CLK_TCK"))
    elapsed_ns = rows[-1]["timestamp_ns"] - rows[0]["timestamp_ns"]
    elapsed_cpu_ticks = rows[-1]["cpu_time_ticks"] - rows[0]["cpu_time_ticks"]
    cpu_percent = 0.0
    if elapsed_ns > 0 and elapsed_cpu_ticks >= 0:
        cpu_percent = (
            elapsed_cpu_ticks / ticks_per_second / (elapsed_ns / 1_000_000_000) * 100
        )
    return {
        "scope": scope,
        "cpu_percent": cpu_percent,
        "max_rss_bytes": max(row["rss_bytes"] for row in rows),
    }


class ProcessResourceSampler:
    FIELDNAMES = (
        "timestamp_ns",
        "cpu_time_ticks",
        "rss_bytes",
        "process_count",
        "thread_count",
        "rss_anon_bytes",
        "rss_file_bytes",
        "vm_data_bytes",
        "vm_size_bytes",
        "pss_anon_bytes",
        "private_dirty_bytes",
    )

    def __init__(
        self,
        path: Path,
        pids: list[int],
        interval_secs: float = 1.0,
        proc_root: Path = Path("/proc"),
    ):
        self.path = path
        self.pids = pids
        self.interval_secs = interval_secs
        self.proc_root = proc_root
        self.stop_event = threading.Event()
        self.error: Exception | None = None
        self.thread: threading.Thread | None = None

    @staticmethod
    def _memory_kib(path: Path, fields: dict[str, str]) -> dict[str, int]:
        values = {name: 0 for name in fields.values()}
        for line in path.read_text().splitlines():
            key, separator, raw = line.partition(":")
            if not separator or key not in fields:
                continue
            values[fields[key]] = int(raw.strip().split()[0])
        return values

    def _sample(self) -> dict[str, int]:
        sample = {field: 0 for field in self.FIELDNAMES}
        sample["timestamp_ns"] = time.time_ns()
        page_size = int(os.sysconf("SC_PAGE_SIZE"))
        for pid in self.pids:
            process = self.proc_root / str(pid)
            try:
                stat = (process / "stat").read_text()
                fields = stat[stat.rfind(")") + 2 :].split()
                resident_pages = int((process / "statm").read_text().split()[1])
                sample["cpu_time_ticks"] += int(fields[11]) + int(fields[12])
                sample["rss_bytes"] += resident_pages * page_size
                sample["thread_count"] += sum(1 for _ in (process / "task").iterdir())
                sample["process_count"] += 1
                status = self._memory_kib(
                    process / "status",
                    {
                        "RssAnon": "rss_anon_bytes",
                        "RssFile": "rss_file_bytes",
                        "VmData": "vm_data_bytes",
                        "VmSize": "vm_size_bytes",
                    },
                )
                smaps = self._memory_kib(
                    process / "smaps_rollup",
                    {
                        "Pss_Anon": "pss_anon_bytes",
                        "Private_Dirty": "private_dirty_bytes",
                    },
                )
                for field, value_kib in status.items() | smaps.items():
                    sample[field] += value_kib * 1024
            except (OSError, IndexError, ValueError):
                continue
        return sample

    def _run(self) -> None:
        try:
            with self.path.open("w", newline="") as stream:
                writer = csv.DictWriter(stream, fieldnames=self.FIELDNAMES)
                writer.writeheader()
                while True:
                    writer.writerow(self._sample())
                    stream.flush()
                    if self.stop_event.wait(self.interval_secs):
                        writer.writerow(self._sample())
                        stream.flush()
                        break
        except (OSError, ValueError) as error:
            self.error = error

    def start(self) -> None:
        self.thread = threading.Thread(target=self._run, daemon=True)
        self.thread.start()

    def stop(self) -> None:
        self.stop_event.set()
        if self.thread is not None:
            self.thread.join(timeout=max(5.0, self.interval_secs * 2))
            if self.thread.is_alive():
                raise RuntimeError("process resource sampler did not stop")
        if self.error is not None:
            raise RuntimeError(f"process resource sampler failed: {self.error}")


def validate_startup_artifact(result: dict) -> None:
    required = {
        "schema_version",
        "clock",
        "cache_state",
        "cache_preparation",
        "compiled_component_cache",
        "plugin_sha256",
        "processed_messages",
        "phases_ns",
        "total_wall_duration_ns",
        "harness_overhead_tolerance_ns",
    }
    missing = sorted(required - result.keys())
    if missing:
        raise ValueError(f"startup artifact missing fields: {', '.join(missing)}")
    if result["schema_version"] != 1 or result["clock"] != "monotonic":
        raise ValueError("startup artifact must use schema version 1 and monotonic durations")
    if result["cache_state"] not in {"cold", "warm"}:
        raise ValueError("startup cache_state must be cold or warm")

    preparation = result["cache_preparation"]
    expected_action = (
        "drop-linux-page-cache" if result["cache_state"] == "cold" else "none"
    )
    if preparation != {
        "action": expected_action,
        "completed_before_timing": True,
    }:
        raise ValueError("startup cache preparation does not match cache_state")

    compiled_cache = result["compiled_component_cache"]
    for field in ("mode", "hit", "artifact", "identity"):
        if field not in compiled_cache:
            raise ValueError(f"compiled component cache missing field: {field}")
    if compiled_cache["hit"] and not (
        compiled_cache["artifact"] and compiled_cache["identity"]
    ):
        raise ValueError("compiled component cache hit requires artifact and identity")
    if compiled_cache["mode"] == "disabled" and (
        compiled_cache["hit"]
        or compiled_cache["artifact"] is not None
        or compiled_cache["identity"] is not None
    ):
        raise ValueError("disabled compiled component cache cannot report hit evidence")

    hashes = result["plugin_sha256"]
    if not isinstance(hashes, dict) or not hashes:
        raise ValueError("startup artifact requires at least one plugin SHA-256")
    if any(
        not isinstance(value, str)
        or len(value) != 64
        or any(char not in "0123456789abcdef" for char in value.lower())
        for value in hashes.values()
    ):
        raise ValueError("startup plugin SHA-256 values must be 64 hexadecimal characters")
    if result["processed_messages"] != 1:
        raise ValueError("startup artifact must contain exactly one processed message")

    phases = result["phases_ns"]
    missing_phases = [phase for phase in STARTUP_PHASES if phase not in phases]
    if missing_phases:
        raise ValueError(f"missing startup phase: {', '.join(missing_phases)}")
    durations = [phases[phase] for phase in STARTUP_PHASES]
    if any(not isinstance(value, int) or value < 0 for value in durations):
        raise ValueError("startup phase durations must be non-negative integer nanoseconds")
    total = result["total_wall_duration_ns"]
    if not isinstance(total, int) or total <= 0:
        raise ValueError("startup total wall duration must be positive integer nanoseconds")
    phase_total = sum(durations)
    if phase_total > total:
        raise ValueError("startup phases exceed total wall duration")
    if result["harness_overhead_tolerance_ns"] != STARTUP_HARNESS_OVERHEAD_TOLERANCE_NS:
        raise ValueError("startup harness-overhead tolerance differs from the frozen value")
    if total - phase_total > STARTUP_HARNESS_OVERHEAD_TOLERANCE_NS:
        raise ValueError("startup unmeasured harness overhead exceeds tolerance")


def validate_rate_sweep_result(result: dict) -> None:
    required = {
        "schema_version",
        "experiment",
        "system",
        "thesis_evidence",
        "measurement_boundary",
        "units",
        "offered_rate_msg_s",
        "actual_offered_rate_msg_s",
        "achieved_rate_msg_s",
        "measurement_duration_ns",
        "messages",
        "loss_percent",
        "latency_ns",
        "resources",
        "throttled",
        "profile",
        "process_audit",
        "traces",
    }
    missing = sorted(required - result.keys())
    if missing:
        raise ValueError(f"rate-sweep result missing fields: {', '.join(missing)}")

    nested = {
        "messages": {"offered", "received", "lost", "duplicates"},
        "latency_ns": {"p50", "p95", "p99"},
        "resources": {"scope", "cpu_percent", "max_rss_bytes"},
        "profile": {"path", "sha256", "payload_template_sha256"},
        "process_audit": {"path", "sha256"},
        "traces": {"published", "received"},
    }
    for section, fields in nested.items():
        value = result.get(section)
        if not isinstance(value, dict):
            raise ValueError(f"rate-sweep result {section} must be an object")
        absent = sorted(fields - value.keys())
        if absent:
            raise ValueError(
                f"rate-sweep result {section} missing fields: {', '.join(absent)}"
            )

    if result["experiment"] != "e-perf-10":
        raise ValueError("rate-sweep result experiment must be e-perf-10")
    if result["system"] not in RATE_SWEEP_SYSTEMS:
        raise ValueError(f"rate-sweep result has unknown system {result['system']!r}")
    if result["thesis_evidence"] is not False:
        raise ValueError("rate-sweep result must set thesis_evidence=false")
    if result["offered_rate_msg_s"] not in RATE_SWEEP_RATES:
        raise ValueError("rate-sweep result offered_rate_msg_s is not frozen")

    messages = result["messages"]
    numeric_values = (
        result["actual_offered_rate_msg_s"],
        result["achieved_rate_msg_s"],
        result["measurement_duration_ns"],
        result["loss_percent"],
        messages["offered"],
        messages["received"],
        messages["lost"],
        messages["duplicates"],
        result["latency_ns"]["p50"],
        result["latency_ns"]["p95"],
        result["latency_ns"]["p99"],
        result["resources"]["cpu_percent"],
        result["resources"]["max_rss_bytes"],
    )
    if any(not isinstance(value, (int, float)) or value < 0 for value in numeric_values):
        raise ValueError("rate-sweep result numeric fields must be non-negative")
    if messages["lost"] != max(0, messages["offered"] - messages["received"]):
        raise ValueError("rate-sweep result lost count is inconsistent")

    for name, trace in result["traces"].items():
        if not isinstance(trace, dict):
            raise ValueError(f"rate-sweep result trace {name} must be an object")
        absent = {"path", "sha256", "samples"} - trace.keys()
        if absent:
            raise ValueError(
                f"rate-sweep result trace {name} missing fields: {', '.join(sorted(absent))}"
            )
        if not re.fullmatch(r"[0-9a-f]{64}", str(trace["sha256"])):
            raise ValueError(f"rate-sweep result trace {name} has invalid sha256")


def classify_sustainable_throughput(
    samples: list[dict],
    baseline_rate_msg_s: int = RATE_SWEEP_BASELINE,
    p99_multiplier_limit: float = RATE_SWEEP_P99_MULTIPLIER,
    max_loss_percent: float = RATE_SWEEP_MAX_LOSS_PERCENT,
) -> dict:
    by_rate: dict[int, list[dict]] = {}
    for sample in samples:
        by_rate.setdefault(int(sample["offered_rate_msg_s"]), []).append(sample)
    if baseline_rate_msg_s not in by_rate:
        raise ValueError(f"missing {baseline_rate_msg_s} msg/s baseline")

    baseline_p99_ns = statistics.median(
        int(sample["p99_ns"]) for sample in by_rate[baseline_rate_msg_s]
    )
    threshold_p99_ns = baseline_p99_ns * p99_multiplier_limit
    rates = []
    for rate, rate_samples in sorted(by_rate.items()):
        p99_ns = statistics.median(int(sample["p99_ns"]) for sample in rate_samples)
        offered = sum(int(sample["offered"]) for sample in rate_samples)
        lost = sum(int(sample["lost"]) for sample in rate_samples)
        loss_percent = 100.0 * lost / offered if offered else 100.0
        breaches = []
        if p99_ns > threshold_p99_ns:
            breaches.append("p99")
        if loss_percent > max_loss_percent:
            breaches.append("loss")
        rates.append(
            {
                "offered_rate_msg_s": rate,
                "sample_count": len(rate_samples),
                "median_p99_ns": p99_ns,
                "loss_percent": loss_percent,
                "breaches": breaches,
                "sustainable": not breaches,
            }
        )

    last_good = None
    first_bad = None
    for rate in (entry for entry in rates if entry["offered_rate_msg_s"] >= baseline_rate_msg_s):
        if rate["sustainable"] and first_bad is None:
            last_good = rate["offered_rate_msg_s"]
        elif not rate["sustainable"] and first_bad is None:
            first_bad = rate["offered_rate_msg_s"]

    return {
        "baseline_rate_msg_s": baseline_rate_msg_s,
        "baseline_p99_ns": baseline_p99_ns,
        "p99_threshold_ns": threshold_p99_ns,
        "max_loss_percent": max_loss_percent,
        "last_good_rate_msg_s": last_good,
        "first_bad_rate_msg_s": first_bad,
        "highest_tested_rate_msg_s": max(by_rate),
        "no_saturation_within_range": first_bad is None,
        "rates": rates,
    }


def utc_now() -> str:
    return dt.datetime.now(dt.timezone.utc).strftime("%Y-%m-%dT%H:%M:%SZ")


def write_progress(
    ledger: Path,
    event: str,
    completed: int,
    total: int,
    item: str = "",
    failures: int = 0,
) -> None:
    try:
        temperature_c = int(
            Path("/sys/class/thermal/thermal_zone0/temp").read_text().strip()
        ) / 1000
    except (OSError, ValueError):
        temperature_c = None
    entry = {
        "timestamp": utc_now(),
        "event": event,
        "completed": completed,
        "total": total,
        "item": item,
        "failures": failures,
        "temperature_c": temperature_c,
    }
    with (ledger / "progress.jsonl").open("a") as stream:
        stream.write(json.dumps(entry, separators=(",", ":")) + "\n")
    print(
        f"[{entry['timestamp']}] PROGRESS {completed}/{total} event={event} "
        f"item={item or '-'} failures={failures} temp_c={temperature_c}",
        flush=True,
    )


def write_status(path: Path, item: RunItem, status: str, detail: str = "") -> None:
    path.mkdir(parents=True, exist_ok=True)
    receipt = {
        "status": status,
        "experiment": item.experiment,
        "condition": item.condition,
        "run_index": item.run_index,
        "updated_at": utc_now(),
    }
    if detail:
        receipt["detail"] = detail
    (path / "canonical-status.json").write_text(json.dumps(receipt, indent=2) + "\n")


def find_passed_attempt(condition_dir: Path, run_index: int) -> Path | None:
    selection = select_attempt(condition_dir, run_index)
    return selection.path if selection.skip else None


def stamp_focused_metadata(root: Path, metadata: dict) -> None:
    matrix_sha256 = os.environ.get(FOCUSED_MATRIX_SHA_ENV)
    if matrix_sha256 is None:
        return
    focused = json.loads((root / "eval/canonical-matrix.json").read_text())["focused_pilot"]
    decisions = focused["decisions"]
    metadata["thesis_evidence"] = False
    metadata["focused_pilot"] = {
        "id": focused["id"],
        "matrix_sha256": matrix_sha256,
        "memory_retention_fix_commit": decisions["memory_retention"]["fix_commit"],
        "ekuiper_operator_concurrency": decisions["ekuiper_operator_concurrency"]["value"],
    }


def copy_shared_result(root: Path, batch_id: str, item: RunItem) -> Path:
    if item.shared_from is None:
        raise ValueError("shared result has no source experiment")
    source_dir = (
        root
        / "eval/results"
        / item.shared_from
        / f"rpi5-{batch_id}"
        / item.condition
    )
    source = find_passed_attempt(source_dir, item.run_index)
    if source is None:
        raise RuntimeError(f"shared source is incomplete: {source_dir}")
    target_dir = (
        root
        / "eval/results"
        / item.experiment
        / f"rpi5-{batch_id}"
        / item.condition
    )
    selection = select_attempt(target_dir, item.run_index)
    if selection.skip:
        return selection.path
    shutil.copytree(source, selection.path)
    status_path = selection.path / "canonical-status.json"
    status_path.unlink(missing_ok=True)
    metadata_path = selection.path / "metadata.json"
    metadata = json.loads(metadata_path.read_text())
    source_leaf = str(source.relative_to(root))
    metadata["experiment"] = item.experiment
    metadata["shared_from"] = source_leaf
    metadata["measurement_source_leaf"] = source_leaf
    metadata["shared_measurement"] = True
    stamp_focused_metadata(root, metadata)
    metadata_path.write_text(json.dumps(metadata, indent=2) + "\n")
    analysis_path = selection.path / "hotswap-analysis.json"
    if analysis_path.is_file():
        analysis = json.loads(analysis_path.read_text())
        analysis["experiment"] = item.experiment
        analysis["shared_from"] = source_leaf
        analysis["measurement_source_leaf"] = source_leaf
        analysis_path.write_text(json.dumps(analysis, indent=2) + "\n")
    return selection.path


def start_pi_telemetry(root: Path, output: Path) -> subprocess.Popen:
    return subprocess.Popen(
        [sys.executable, str(root / "eval/scripts/lib/pi_telemetry.py"), str(output)],
        cwd=root,
    )


def stop_pi_telemetry(process: subprocess.Popen | None) -> None:
    if process is None or process.poll() is not None:
        return
    process.terminate()
    try:
        process.wait(timeout=5)
    except subprocess.TimeoutExpired:
        process.kill()
        process.wait(timeout=5)


def wait_for_api(url: str, timeout_secs: float = 10.0) -> None:
    deadline = time.monotonic() + timeout_secs
    while time.monotonic() < deadline:
        try:
            with urllib.request.urlopen(url, timeout=1):
                return
        except (OSError, urllib.error.URLError):
            time.sleep(0.1)
    raise RuntimeError(f"API did not become ready: {url}")


def post_hot_swap(node_id: str, plugin: Path) -> dict:
    request = urllib.request.Request(
        f"http://127.0.0.1:9090/api/v1/nodes/{node_id}/hot-swap",
        data=json.dumps({"wasm_path": str(plugin)}).encode(),
        headers={"Content-Type": "application/json"},
        method="POST",
    )
    try:
        with urllib.request.urlopen(request, timeout=30) as response:
            body = response.read().decode()
            return {"http_status": response.status, "body": json.loads(body)}
    except urllib.error.HTTPError as error:
        body = error.read().decode(errors="replace")
        return {"http_status": error.code, "body": body}


def run_hot_swap_item(root: Path, item: RunItem, selection: AttemptSelection) -> bool:
    output = selection.path
    output.mkdir(parents=True)
    config = root / item.config
    shutil.copy2(config, output / "config.toml")
    print(f"[{utc_now()}] START {item.result_key} -> {output}", flush=True)
    started_ns = time.time_ns()
    started_at = utc_now()
    runtime: subprocess.Popen | None = None
    telemetry = start_pi_telemetry(root, output)
    try:
        set_ekuiper_active(root, False)
        subprocess.run(
            [
                sys.executable,
                str(root / "eval/scripts/validate-canonical.py"),
                "host",
                "--root",
                str(root),
            ],
            cwd=root,
            check=True,
        )
        environment = os.environ.copy()
        environment["WAFER_BENCH_OUTPUT_DIR"] = str(output)
        with (output / "stdout.log").open("ab") as log:
            runtime = subprocess.Popen(
                [
                    "taskset", "-c", item.runtime_cpus,
                    str(root / "target/release/wafer"), "--config", str(config),
                ],
                cwd=root,
                env=environment,
                stdout=log,
                stderr=log,
            )
            wait_for_api("http://127.0.0.1:9090/health")
            time.sleep(item.warmup_secs)
            v1 = root / "plugins/pass-through-v1/target/wasm32-wasip2/release/wafer_pass_through_v1.wasm"
            v2 = root / "plugins/pass-through-v2/target/wasm32-wasip2/release/wafer_pass_through_v2.wasm"
            panics = root / "plugins/pass-through-v2-panics/target/wasm32-wasip2/release/wafer_pass_through_v2_panics.wasm"
            event_count = item.events_per_run or 50
            interval = item.measurement_secs / event_count
            requests: list[dict] = []
            for event_index in range(event_count):
                event_started = time.monotonic()
                plugin = panics if item.experiment == "e-swap-5" else (v2 if event_index % 2 == 0 else v1)
                request_started_ns = time.time_ns()
                request_started_monotonic_ns = time.monotonic_ns()
                response = post_hot_swap("transform", plugin)
                request_duration_ns = time.monotonic_ns() - request_started_monotonic_ns
                request_finished_ns = time.time_ns()
                requests.append(
                    {
                        "event_index": event_index,
                        "plugin": plugin.name,
                        "request_started_ns": request_started_ns,
                        "request_finished_ns": request_finished_ns,
                        "request_timestamp_clock": "unix-epoch",
                        "request_duration_ns": request_duration_ns,
                        "request_duration_clock": "monotonic",
                        **response,
                    }
                )
                remaining = interval - (time.monotonic() - event_started)
                if remaining > 0:
                    time.sleep(remaining)
            (output / "swap_requests.json").write_text(json.dumps(requests, indent=2) + "\n")
            try:
                runtime_exit = runtime.wait(timeout=30)
            except subprocess.TimeoutExpired:
                runtime.terminate()
                runtime_exit = runtime.wait(timeout=10)
            runtime = None
        if runtime_exit != 0:
            raise RuntimeError(f"wafer runtime exited with {runtime_exit}")
        if not (output / "swap_timeline.json").is_file():
            (output / "swap_timeline.json").write_text(
                json.dumps({"requests": requests}, indent=2) + "\n"
            )
        if item.experiment == "e-swap-5":
            rolled_back = sum(
                1
                for request in requests
                if request["http_status"] == 200
                and isinstance(request["body"], dict)
                and request["body"].get("status") == "rolled_back"
            )
            rollback = {
                "attempts": len(requests),
                "rolled_back": rolled_back,
                "all_rolled_back": rolled_back == len(requests),
            }
            (output / "rollback.json").write_text(json.dumps(rollback, indent=2) + "\n")
            if not rollback["all_rolled_back"]:
                raise RuntimeError(
                    f"only {rolled_back}/{len(requests)} failed swaps rolled back"
                )
        else:
            successful = sum(request["http_status"] == 200 for request in requests)
            if successful != len(requests):
                raise RuntimeError(f"only {successful}/{len(requests)} hot swaps succeeded")

        with (output / "sequence.csv").open(newline="") as stream:
            sequence = next(csv.DictReader(stream))
        if int(sequence["gap_msgs"]) != 0 or int(sequence["duplicates_count"]) != 0:
            raise RuntimeError(
                "hot-swap sequence integrity failed: "
                f"gaps={sequence['gap_msgs']}, duplicates={sequence['duplicates_count']}"
            )
        stop_pi_telemetry(telemetry)
        finished_ns = time.time_ns()
        provenance_path = output / "runtime-provenance.json"
        provenance = provenance_path.read_text() if provenance_path.is_file() else "null"
        merge_metadata(
            str(output / "metadata.json"),
            item.experiment,
            "rpi5",
            utc_now(),
            started_at,
            str(finished_ns - started_ns),
            item.config,
            hashlib.sha256(config.read_bytes()).hexdigest(),
            "null",
            json.dumps({"broker": "127.0.0.1:1883", "managed_by_harness": False}),
            str(runtime_exit),
            provenance,
        )
        postprocess_run(root, item, output)
        verify_result(root, output)
    except (OSError, ValueError, RuntimeError, subprocess.CalledProcessError, subprocess.TimeoutExpired) as error:
        if runtime is not None and runtime.poll() is None:
            runtime.terminate()
        stop_pi_telemetry(telemetry)
        write_status(output, item, "failed", str(error))
        print(f"[{utc_now()}] FAIL {item.result_key}: {error}", flush=True)
        return False
    write_status(output, item, "passed")
    print(f"[{utc_now()}] PASS {item.result_key}", flush=True)
    return True


def wait_for_subscriber(process: subprocess.Popen, timeout: int = 30) -> int:
    try:
        return process.wait(timeout=timeout)
    except subprocess.TimeoutExpired:
        process.send_signal(signal.SIGINT)
        return process.wait(timeout=10)


def loadgen_command(
    root: Path,
    item: RunItem,
    action: str,
    output: Path | None = None,
    duration: int | None = None,
    topic: str | None = None,
    trace_file: Path | None = None,
    sequence_start: int | None = None,
) -> list[str]:
    loadgen = root / "target/release/wafer-loadgen"
    if action == "subscribe":
        if output is None:
            raise ValueError("subscriber requires an output directory")
        command = [
            "taskset", "-c", item.support_cpus,
            str(loadgen), "subscribe", "--broker", "127.0.0.1:1883",
            "--topic", topic or "wafer/telemetry/hot", "--output-dir", str(output),
            "--total-messages", str(item.total_messages or 0),
            "--host-tag", "rpi5",
        ]
        if item.experiment == "e-perf-10" and item.total_messages is not None:
            command.extend(["--sequence-end-exclusive", str(item.total_messages)])
        if trace_file is not None:
            command.extend(["--trace-file", str(trace_file)])
        return command
    command = [
        "taskset", "-c", item.support_cpus,
        str(loadgen), "publish",
        "--broker-host", "127.0.0.1", "--broker-port", "1883",
        "--topic", topic or "wafer/telemetry", "--profile-file", str(root / str(item.loadgen_profile)),
        "--duration-secs", str(duration if duration is not None else item.measurement_secs),
    ]
    if item.offered_rate_msg_s is not None:
        command.extend(["--rate", str(item.offered_rate_msg_s)])
    if item.experiment == "e-perf-10":
        command.append("--drop-when-full")
    if sequence_start is not None:
        command.extend(["--sequence-start", str(sequence_start)])
    if trace_file is not None:
        command.extend(["--trace-file", str(trace_file)])
    return command


def run_restart_item(
    root: Path,
    item: RunItem,
    selection: AttemptSelection,
) -> bool:
    output = selection.path
    output.mkdir(parents=True)
    config = root / item.config
    shutil.copy2(config, output / "config.toml")
    print(f"[{utc_now()}] START {item.result_key} -> {output}", flush=True)
    started_ns = time.time_ns()
    started_at = utc_now()
    runtime: subprocess.Popen | None = None
    runtime_exit = 0
    ekuiper_audit: Path | None = None
    telemetry = start_pi_telemetry(root, output)
    try:
        is_ekuiper = item.system == "ekuiper"
        set_ekuiper_active(root, is_ekuiper)
        facts_path = output / "host-facts.json"
        host_command = [
            sys.executable,
            str(root / "eval/scripts/validate-canonical.py"),
            "host",
            "--root",
            str(root),
            "--output",
            str(facts_path),
        ]
        if is_ekuiper:
            host_command.append("--require-ekuiper")
        subprocess.run(host_command, cwd=root, check=True)
        facts = json.loads(facts_path.read_text())
        if is_ekuiper:
            ekuiper_audit = capture_ekuiper_audit(root, output, item.runtime_cpus)
        environment = os.environ.copy()
        environment["WAFER_GIT_SHA"] = facts["git_sha"]
        environment["WAFER_BENCH_OUTPUT_DIR"] = str(output)

        with (output / "stdout.log").open("ab") as log:
            runtime_command = [
                "taskset", "-c", item.runtime_cpus,
                str(root / "target/release/wafer"), "--config", str(config),
            ]
            if not is_ekuiper:
                runtime = subprocess.Popen(
                    runtime_command,
                    cwd=root,
                    env=environment,
                    stdout=log,
                    stderr=log,
                )
                time.sleep(1)
                if runtime.poll() is not None:
                    raise RuntimeError("wafer runtime exited during startup")

            subprocess.run(
                loadgen_command(root, item, "publish", duration=item.warmup_secs),
                cwd=root,
                env=environment,
                stdout=log,
                stderr=log,
                check=True,
            )
            measurement_started_ns = time.time_ns()
            subscriber = subprocess.Popen(
                loadgen_command(root, item, "subscribe", output=output),
                cwd=root,
                env=environment,
                stdout=log,
                stderr=log,
            )
            time.sleep(0.5)
            publisher = subprocess.Popen(
                loadgen_command(root, item, "publish"),
                cwd=root,
                env=environment,
                stdout=log,
                stderr=log,
            )
            time.sleep(item.measurement_secs / 2)
            action_started_ns = time.time_ns()
            if is_ekuiper:
                subprocess.run(
                    ["curl", "-fsS", "-X", "POST", "http://127.0.0.1:9081/rules/pipeline_a/stop"],
                    stdout=log,
                    stderr=log,
                    check=True,
                )
                subprocess.run(
                    ["curl", "-fsS", "-X", "POST", "http://127.0.0.1:9081/rules/pipeline_a/start"],
                    stdout=log,
                    stderr=log,
                    check=True,
                )
            else:
                assert runtime is not None
                runtime.terminate()
                runtime.wait(timeout=10)
                runtime = subprocess.Popen(
                    runtime_command,
                    cwd=root,
                    env=environment,
                    stdout=log,
                    stderr=log,
                )
                time.sleep(1)
                if runtime.poll() is not None:
                    raise RuntimeError("wafer runtime failed to restart")
            action_finished_ns = time.time_ns()
            publisher_code = publisher.wait(timeout=item.measurement_secs + 30)
            subscriber_code = wait_for_subscriber(subscriber)
            if publisher_code != 0 or subscriber_code != 0:
                raise RuntimeError(
                    f"loadgen failed: publisher={publisher_code}, subscriber={subscriber_code}"
                )
            measurement_finished_ns = time.time_ns()

        (output / "measurement-window.json").write_text(
            json.dumps(
                {
                    "started_ns": measurement_started_ns,
                    "finished_ns": measurement_finished_ns,
                },
                indent=2,
            )
            + "\n"
        )
        if runtime is not None:
            runtime.terminate()
            try:
                runtime_exit = runtime.wait(timeout=10)
            except subprocess.TimeoutExpired:
                runtime.kill()
                runtime_exit = runtime.wait(timeout=5)
            runtime = None

        (output / "swap_timeline.json").write_text(
            json.dumps(
                {
                    "strategy": item.condition,
                    "action_started_ns": action_started_ns,
                    "action_finished_ns": action_finished_ns,
                    "action_duration_ns": action_finished_ns - action_started_ns,
                },
                indent=2,
            )
            + "\n"
        )
        stop_pi_telemetry(telemetry)
        finished_ns = time.time_ns()
        config_sha = hashlib.sha256(config.read_bytes()).hexdigest()
        if is_ekuiper:
            metadata = {
                "experiment": item.experiment,
                "system": "ekuiper",
                "condition": item.condition,
                "run_index": item.run_index,
                "host_tag": "rpi5",
                "generated_at": utc_now(),
                "started_at": started_at,
                "duration_ns": finished_ns - started_ns,
                "config_path": item.config,
                "config_sha256": config_sha,
                "loadgen": {"profile_path": item.loadgen_profile, "warmup_secs": item.warmup_secs},
                "exit_codes": {"ekuiper": 0},
                "comparator_audit": {
                    "path": ekuiper_audit.name,
                    "sha256": hashlib.sha256(ekuiper_audit.read_bytes()).hexdigest(),
                },
                **facts,
            }
            (output / "metadata.json").write_text(json.dumps(metadata, indent=2) + "\n")
        else:
            provenance_path = output / "runtime-provenance.json"
            provenance = provenance_path.read_text() if provenance_path.is_file() else "null"
            merge_metadata(
                str(output / "metadata.json"),
                item.experiment,
                "rpi5",
                utc_now(),
                started_at,
                str(finished_ns - started_ns),
                item.config,
                config_sha,
                json.dumps({"profile_path": item.loadgen_profile, "warmup_secs": item.warmup_secs}),
                json.dumps({"broker": "127.0.0.1:1883", "managed_by_harness": False}),
                str(runtime_exit),
                provenance,
            )
        postprocess_run(root, item, output)
        verify_result(root, output)
    except (OSError, ValueError, RuntimeError, subprocess.CalledProcessError, subprocess.TimeoutExpired) as error:
        if runtime is not None and runtime.poll() is None:
            runtime.terminate()
        stop_pi_telemetry(telemetry)
        write_status(output, item, "failed", str(error))
        print(f"[{utc_now()}] FAIL {item.result_key}: {error}", flush=True)
        return False
    write_status(output, item, "passed")
    print(f"[{utc_now()}] PASS {item.result_key}", flush=True)
    return True


def _expand_cpu_list(value: str) -> set[int]:
    cpus: set[int] = set()
    for part in value.split(","):
        bounds = part.strip().split("-", maxsplit=1)
        if len(bounds) == 1:
            cpus.add(int(bounds[0]))
        else:
            cpus.update(range(int(bounds[0]), int(bounds[1]) + 1))
    return cpus


def validate_ekuiper_process_snapshot(snapshot: dict, allowed_cpus: str) -> None:
    processes = snapshot.get("processes", [])
    if not processes or snapshot.get("main_pid") not in {process.get("pid") for process in processes}:
        raise ValueError("eKuiper main PID is absent from process snapshot")
    if snapshot.get("other_suts"):
        raise ValueError(f"concurrent SUT processes detected: {snapshot['other_suts']}")
    allowed = _expand_cpu_list(allowed_cpus)
    for process in processes:
        observed = _expand_cpu_list(str(process.get("cpus_allowed_list", "")))
        if not observed or not observed <= allowed:
            raise ValueError(
                f"eKuiper PID {process.get('pid')} affinity {sorted(observed)} is outside {allowed_cpus}"
            )


def _process_snapshot(main_pid: int) -> dict:
    output = subprocess.check_output(
        ["ps", "-e", "-o", "pid=,ppid=,comm=,args="], text=True
    )
    rows = []
    for line in output.splitlines():
        fields = line.strip().split(maxsplit=3)
        if len(fields) < 3:
            continue
        rows.append(
            {
                "pid": int(fields[0]),
                "ppid": int(fields[1]),
                "name": Path(fields[2]).name,
                "args": fields[3] if len(fields) == 4 else "",
            }
        )
    descendants = {main_pid}
    while True:
        children = {row["pid"] for row in rows if row["ppid"] in descendants}
        expanded = descendants | children
        if expanded == descendants:
            break
        descendants = expanded
    processes = []
    for row in rows:
        if row["pid"] not in descendants:
            continue
        status = Path(f"/proc/{row['pid']}/status").read_text()
        affinity = next(
            line.split(":", maxsplit=1)[1].strip()
            for line in status.splitlines()
            if line.startswith("Cpus_allowed_list:")
        )
        processes.append({**row, "cpus_allowed_list": affinity})
    other_suts = [
        row for row in rows if row["name"] in {"wafer", "wafer-runtime"}
    ]
    return {"main_pid": main_pid, "processes": processes, "other_suts": other_suts}


def _service_properties() -> dict[str, str]:
    properties = (
        "MainPID",
        "User",
        "Group",
        "ExecStart",
        "Environment",
        "CPUAffinity",
        "Restart",
        "RestartUSec",
        "WorkingDirectory",
        "FragmentPath",
        "DropInPaths",
    )
    command = ["systemctl", "show", "kuiper.service"]
    command.extend(f"--property={name}" for name in properties)
    output = subprocess.check_output(command, text=True)
    return dict(line.split("=", maxsplit=1) for line in output.splitlines() if "=" in line)


def _url_value(url: str) -> object:
    with urllib.request.urlopen(url, timeout=5) as response:
        text = response.read().decode()
    try:
        return json.loads(text)
    except json.JSONDecodeError:
        return text


def ekuiper_operator_concurrency(root: Path) -> int:
    config = tomllib.loads(
        (root / "eval/configs/canonical/e-perf-1-ekuiper.toml").read_text()
    )
    concurrency = config["comparator"].get("operator_concurrency")
    if not isinstance(concurrency, int) or isinstance(concurrency, bool) or concurrency < 1:
        raise ValueError("eKuiper operator_concurrency must be a positive integer")
    return concurrency


def capture_ekuiper_audit(root: Path, output: Path, allowed_cpus: str) -> Path:
    operator_concurrency = ekuiper_operator_concurrency(root)
    service = _service_properties()
    main_pid = int(service.get("MainPID", "0"))
    snapshot = _process_snapshot(main_pid)
    validate_ekuiper_process_snapshot(snapshot, allowed_cpus)
    unit_text = subprocess.check_output(
        ["systemctl", "cat", "kuiper.service"], text=True
    )
    source_config = Path("/etc/kuiper/mqtt_source.yaml")
    source_text = source_config.read_text()
    expected_source_text = (root / "eval/ekuiper/mqtt-source-default.yaml").read_text()
    if source_text != expected_source_text:
        raise ValueError("eKuiper MQTT source configuration differs from the canonical file")
    install_receipt = json.loads(
        Path("/var/lib/kuiper/wafer-install-receipt.json").read_text()
    )
    version = subprocess.check_output(
        ["dpkg-query", "-W", "-f=${Version}", "kuiper"], text=True
    ).strip()
    if install_receipt.get("version") != version or not re.fullmatch(
        r"[0-9a-f]{64}", str(install_receipt.get("sha256", ""))
    ):
        raise ValueError("eKuiper package version/checksum does not match install receipt")
    rule = _url_value("http://127.0.0.1:9081/rules/pipeline_a")
    if not isinstance(rule, dict) or rule.get("options", {}).get("concurrency") != operator_concurrency:
        raise ValueError("active eKuiper rule does not use the frozen operator concurrency")
    audit = {
        "captured_at": utc_now(),
        "system": "ekuiper",
        "version": version,
        "install_receipt": install_receipt,
        "service": {
            "properties": service,
            "unit_sha256": hashlib.sha256(unit_text.encode()).hexdigest(),
            "unit_text": unit_text,
        },
        "mqtt_source_config": {
            "path": str(source_config),
            "sha256": hashlib.sha256(source_text.encode()).hexdigest(),
            "settings": {
                "server": "tcp://127.0.0.1:1883",
                "qos": 1,
                "protocol_version": "3.1.1",
                "insecure_skip_verify": False,
            },
        },
        "process_snapshot": snapshot,
        "stream": _url_value("http://127.0.0.1:9081/streams/wafer_telemetry"),
        "rule": rule,
        "seed_dry_run": json.loads(
            subprocess.check_output(
                [
                    str(root / "eval/ekuiper/seed-pipeline-a.sh"),
                    "--concurrency",
                    str(operator_concurrency),
                    "--dry-run",
                ],
                cwd=root,
                text=True,
            )
        ),
    }
    path = output / "ekuiper-audit.json"
    path.write_text(json.dumps(audit, indent=2) + "\n")
    return path


def set_ekuiper_active(root: Path, active: bool) -> None:
    action = "start" if active else "stop"
    subprocess.run(["sudo", "systemctl", action, "kuiper.service"], check=True)
    if active:
        subprocess.run(
            [
                str(root / "eval/ekuiper/seed-pipeline-a.sh"),
                "--concurrency",
                str(ekuiper_operator_concurrency(root)),
            ],
            cwd=root,
            check=True,
        )


def run_ekuiper_item(
    root: Path,
    item: RunItem,
    selection: AttemptSelection,
) -> bool:
    output = selection.path
    output.mkdir(parents=True)
    config = root / item.config
    shutil.copy2(config, output / "config.toml")
    print(f"[{utc_now()}] START {item.result_key} -> {output}", flush=True)
    started_ns = time.time_ns()
    started_at = utc_now()
    telemetry = start_pi_telemetry(root, output)
    try:
        set_ekuiper_active(root, True)
        facts_path = output / "host-facts.json"
        subprocess.run(
            [
                sys.executable,
                str(root / "eval/scripts/validate-canonical.py"),
                "host",
                "--root",
                str(root),
                "--require-ekuiper",
                "--output",
                str(facts_path),
            ],
            cwd=root,
            check=True,
        )
        environment = os.environ.copy()
        environment["WAFER_GIT_SHA"] = json.loads(facts_path.read_text())["git_sha"]
        ekuiper_audit = capture_ekuiper_audit(root, output, item.runtime_cpus)
        with (output / "stdout.log").open("ab") as log:
            subprocess.run(
                loadgen_command(root, item, "publish", duration=item.warmup_secs),
                cwd=root,
                env=environment,
                stdout=log,
                stderr=log,
                check=True,
            )
            measurement_started_ns = time.time_ns()
            subscriber = subprocess.Popen(
                loadgen_command(root, item, "subscribe", output=output),
                cwd=root,
                env=environment,
                stdout=log,
                stderr=log,
            )
            time.sleep(0.5)
            publisher = subprocess.run(
                loadgen_command(root, item, "publish"),
                cwd=root,
                env=environment,
                stdout=log,
                stderr=log,
                check=False,
            )
            subscriber_code = wait_for_subscriber(subscriber)
            measurement_finished_ns = time.time_ns()
        if publisher.returncode != 0 or subscriber_code != 0:
            raise RuntimeError(
                f"loadgen failed: publisher={publisher.returncode}, subscriber={subscriber_code}"
            )
        (output / "measurement-window.json").write_text(
            json.dumps(
                {
                    "started_ns": measurement_started_ns,
                    "finished_ns": measurement_finished_ns,
                },
                indent=2,
            )
            + "\n"
        )

        stop_pi_telemetry(telemetry)
        finished_ns = time.time_ns()
        facts = json.loads(facts_path.read_text())
        metadata = {
            "experiment": item.experiment,
            "system": "ekuiper",
            "condition": item.condition,
            "run_index": item.run_index,
            "host_tag": "rpi5",
            "generated_at": utc_now(),
            "started_at": started_at,
            "duration_ns": finished_ns - started_ns,
            "config_path": item.config,
            "config_sha256": hashlib.sha256(config.read_bytes()).hexdigest(),
            "loadgen": {
                "profile_path": item.loadgen_profile,
                "warmup_secs": item.warmup_secs,
            },
            "exit_codes": {"ekuiper": 0},
            "comparator_audit": {
                "path": ekuiper_audit.name,
                "sha256": hashlib.sha256(ekuiper_audit.read_bytes()).hexdigest(),
            },
            **facts,
        }
        (output / "metadata.json").write_text(json.dumps(metadata, indent=2) + "\n")
        postprocess_run(root, item, output)
        verify_result(root, output)
    except (OSError, ValueError, RuntimeError, subprocess.CalledProcessError) as error:
        stop_pi_telemetry(telemetry)
        write_status(output, item, "failed", str(error))
        print(f"[{utc_now()}] FAIL {item.result_key}: {error}", flush=True)
        return False
    write_status(output, item, "passed")
    print(f"[{utc_now()}] PASS {item.result_key}", flush=True)
    return True


def _running_sut_processes() -> list[dict]:
    output = subprocess.check_output(
        ["ps", "-e", "-o", "pid=,ppid=,comm=,args="], text=True
    )
    processes = []
    for line in output.splitlines():
        fields = line.strip().split(maxsplit=3)
        if len(fields) < 3 or Path(fields[2]).name not in {"wafer", "wafer-runtime", "kuiperd"}:
            continue
        pid = int(fields[0])
        status = Path(f"/proc/{pid}/status").read_text()
        affinity = next(
            value.split(":", maxsplit=1)[1].strip()
            for value in status.splitlines()
            if value.startswith("Cpus_allowed_list:")
        )
        processes.append(
            {
                "pid": pid,
                "ppid": int(fields[1]),
                "name": Path(fields[2]).name,
                "args": fields[3] if len(fields) == 4 else "",
                "cpus_allowed_list": affinity,
            }
        )
    return processes


def _write_process_audit(
    output: Path,
    item: RunItem,
    processes: list[dict],
) -> Path:
    allowed = _expand_cpu_list(item.runtime_cpus)
    for process in processes:
        observed = _expand_cpu_list(process["cpus_allowed_list"])
        if not observed or not observed <= allowed:
            raise ValueError(
                f"SUT PID {process['pid']} affinity {sorted(observed)} is outside {item.runtime_cpus}"
            )
    audit = {
        "captured_at": utc_now(),
        "system": item.system,
        "exclusive_sut": item.exclusive_sut,
        "allowed_cpus": item.runtime_cpus,
        "processes": processes,
        "concurrent_suts": len(processes) > 1 and item.system != "ekuiper",
    }
    if audit["concurrent_suts"]:
        raise ValueError(f"concurrent SUT processes detected: {processes}")
    path = output / "process-audit.json"
    path.write_text(json.dumps(audit, indent=2) + "\n")
    return path


def _file_receipt(path: Path) -> dict:
    with path.open() as stream:
        samples = max(0, sum(1 for _ in stream) - 1)
    return {
        "path": path.name,
        "sha256": hashlib.sha256(path.read_bytes()).hexdigest(),
        "samples": samples,
    }


def _rate_sweep_throttled(path: Path) -> bool:
    with path.open(newline="") as stream:
        rows = list(csv.DictReader(stream))
    if not rows:
        raise ValueError("pi-telemetry.csv contains no samples")
    return any(row.get("throttled") != "0x0" for row in rows)


def write_rate_sweep_result(
    root: Path,
    item: RunItem,
    output: Path,
    measurement_duration_ns: int,
) -> dict:
    if measurement_duration_ns <= 0:
        raise ValueError("rate-sweep measurement duration must be positive")
    traces = analyze_rate_sweep_traces(
        output / "published.csv",
        output / "received.csv",
        output / "subscriber-metadata.json",
    )
    messages = traces["messages"]
    loss_percent = (
        100.0 * messages["lost"] / messages["offered"]
        if messages["offered"]
        else 100.0
    )
    profile_path = root / str(item.loadgen_profile)
    profile = tomllib.loads(profile_path.read_text())["loadgen"]
    process_audit = output / "process-audit.json"
    if (output / "telemetry-error.json").exists():
        raise ValueError("Pi telemetry failed during rate-sweep measurement")
    resources = summarize_process_resources(output / "resource-usage.csv")
    expected_scope = "no-sut" if item.system == "mqtt-loopback" else "sut"
    if resources["scope"] != expected_scope:
        raise ValueError(
            f"rate-sweep resource scope {resources['scope']!r}, expected {expected_scope!r}"
        )
    result = {
        "schema_version": 1,
        "experiment": item.experiment,
        "system": item.system,
        "thesis_evidence": False,
        "measurement_boundary": "publisher run window to subscriber receive timestamp",
        "units": {
            "rate": "messages/second",
            "latency": "nanoseconds",
            "duration": "nanoseconds",
            "timestamps": "nanoseconds since Unix epoch",
            "counts": "messages",
            "cpu": "percent of one logical CPU",
            "rss": "bytes",
        },
        "offered_rate_msg_s": item.offered_rate_msg_s,
        "actual_offered_rate_msg_s": messages["offered"] / (measurement_duration_ns / 1_000_000_000),
        "achieved_rate_msg_s": messages["received"] / (measurement_duration_ns / 1_000_000_000),
        "measurement_duration_ns": measurement_duration_ns,
        "messages": messages,
        "loss_percent": loss_percent,
        "latency_ns": traces["latency_ns"],
        "resources": resources,
        "throttled": _rate_sweep_throttled(output / "pi-telemetry.csv"),
        "profile": {
            "path": item.loadgen_profile,
            "sha256": hashlib.sha256(profile_path.read_bytes()).hexdigest(),
            "payload_template_sha256": profile["payload_template_sha256"],
        },
        "process_audit": {
            "path": process_audit.name,
            "sha256": hashlib.sha256(process_audit.read_bytes()).hexdigest(),
        },
        "traces": {
            "published": _file_receipt(output / "published.csv"),
            "received": _file_receipt(output / "received.csv"),
        },
    }
    validate_rate_sweep_result(result)
    (output / "rate-sweep.json").write_text(json.dumps(result, indent=2) + "\n")
    return result


def run_rate_sweep_item(
    root: Path,
    item: RunItem,
    selection: AttemptSelection,
) -> bool:
    output = selection.path
    output.mkdir(parents=True)
    config = root / item.config
    shutil.copy2(config, output / "config.toml")
    print(f"[{utc_now()}] START {item.result_key} -> {output}", flush=True)
    started_ns = time.time_ns()
    started_at = utc_now()
    runtime: subprocess.Popen | None = None
    publisher: subprocess.Popen | None = None
    subscriber: subprocess.Popen | None = None
    sampler: ProcessResourceSampler | None = None
    telemetry = start_pi_telemetry(root, output)
    ekuiper_active = False
    runtime_exit = 0
    ekuiper_audit: Path | None = None
    try:
        set_ekuiper_active(root, False)
        existing = _running_sut_processes()
        if existing:
            raise RuntimeError(f"SUT process already active before rate sweep: {existing}")

        if item.system == "ekuiper":
            set_ekuiper_active(root, True)
            ekuiper_active = True

        facts_path = output / "host-facts.json"
        host_command = [
            sys.executable,
            str(root / "eval/scripts/validate-canonical.py"),
            "host",
            "--root",
            str(root),
            "--output",
            str(facts_path),
        ]
        if item.system == "ekuiper":
            host_command.append("--require-ekuiper")
        subprocess.run(host_command, cwd=root, check=True)
        facts = json.loads(facts_path.read_text())
        environment = os.environ.copy()
        environment["WAFER_GIT_SHA"] = facts["git_sha"]
        environment["WAFER_BENCH_OUTPUT_DIR"] = str(output)

        if item.system in {"wafer", "native"}:
            with (output / "stdout.log").open("ab") as log:
                runtime = subprocess.Popen(
                    [
                        "taskset",
                        "-c",
                        item.runtime_cpus,
                        str(root / "target/release/wafer"),
                        "--config",
                        str(config),
                    ],
                    cwd=root,
                    env=environment,
                    stdout=log,
                    stderr=log,
                )
            time.sleep(1)
            if runtime.poll() is not None:
                raise RuntimeError("wafer runtime exited during startup")
            processes = _running_sut_processes()
            if {process["pid"] for process in processes} != {runtime.pid}:
                raise RuntimeError(f"unexpected active SUT processes: {processes}")
        elif item.system == "ekuiper":
            ekuiper_audit = capture_ekuiper_audit(root, output, item.runtime_cpus)
            processes = json.loads(ekuiper_audit.read_text())["process_snapshot"]["processes"]
        else:
            processes = []
        _write_process_audit(output, item, processes)

        input_topic = "wafer/telemetry"
        output_topic = input_topic if item.system == "mqtt-loopback" else "wafer/telemetry/hot"
        with (output / "stdout.log").open("ab") as log:
            subprocess.run(
                loadgen_command(
                    root,
                    item,
                    "publish",
                    duration=item.warmup_secs,
                    topic=input_topic,
                    sequence_start=item.total_messages,
                ),
                cwd=root,
                env=environment,
                stdout=log,
                stderr=log,
                check=True,
            )
            subscriber = subprocess.Popen(
                loadgen_command(
                    root,
                    item,
                    "subscribe",
                    output=output,
                    topic=output_topic,
                    trace_file=output / "received.csv",
                ),
                cwd=root,
                env=environment,
                stdout=log,
                stderr=log,
            )
            time.sleep(0.5)
            sampler = ProcessResourceSampler(
                output / "resource-usage.csv",
                [process["pid"] for process in processes],
            )
            sampler.start()
            measurement_started_ns = time.time_ns()
            publisher = subprocess.Popen(
                loadgen_command(
                    root,
                    item,
                    "publish",
                    topic=input_topic,
                    trace_file=output / "published.csv",
                ),
                cwd=root,
                env=environment,
                stdout=log,
                stderr=log,
            )
            publisher_code = publisher.wait(timeout=item.measurement_secs + 30)
            measurement_finished_ns = time.time_ns()
            if subscriber.poll() is None:
                subscriber.send_signal(signal.SIGINT)
            subscriber_code = subscriber.wait(timeout=10)
        sampler.stop()
        sampler = None
        if publisher_code != 0 or subscriber_code != 0:
            raise RuntimeError(
                f"loadgen failed: publisher={publisher_code}, subscriber={subscriber_code}"
            )
        measurement_duration_ns = measurement_finished_ns - measurement_started_ns
        (output / "measurement-window.json").write_text(
            json.dumps(
                {"started_ns": measurement_started_ns, "finished_ns": measurement_finished_ns},
                indent=2,
            )
            + "\n"
        )

        if runtime is not None:
            runtime.terminate()
            try:
                runtime_exit = runtime.wait(timeout=10)
            except subprocess.TimeoutExpired:
                runtime.kill()
                runtime_exit = runtime.wait(timeout=5)
            runtime = None
            if runtime_exit != 0:
                raise RuntimeError(f"wafer runtime exited with {runtime_exit}")
        if ekuiper_active:
            set_ekuiper_active(root, False)
            ekuiper_active = False
        stop_pi_telemetry(telemetry)
        telemetry = None

        finished_ns = time.time_ns()
        config_sha = hashlib.sha256(config.read_bytes()).hexdigest()
        if item.system in {"ekuiper", "mqtt-loopback"}:
            exit_codes = (
                {"ekuiper": 0}
                if item.system == "ekuiper"
                else {"publisher": publisher_code, "subscriber": subscriber_code}
            )
            metadata = {
                "experiment": item.experiment,
                "system": item.system,
                "condition": item.condition,
                "run_index": item.run_index,
                "host_tag": "rpi5",
                "generated_at": utc_now(),
                "started_at": started_at,
                "duration_ns": finished_ns - started_ns,
                "config_path": item.config,
                "config_sha256": config_sha,
                "loadgen": {
                    "profile_path": item.loadgen_profile,
                    "warmup_secs": item.warmup_secs,
                    "offered_rate_msg_s": item.offered_rate_msg_s,
                },
                "exit_codes": exit_codes,
                **facts,
            }
            if ekuiper_audit is not None:
                metadata["comparator_audit"] = {
                    "path": ekuiper_audit.name,
                    "sha256": hashlib.sha256(ekuiper_audit.read_bytes()).hexdigest(),
                }
            (output / "metadata.json").write_text(json.dumps(metadata, indent=2) + "\n")
        else:
            provenance_path = output / "runtime-provenance.json"
            provenance = provenance_path.read_text() if provenance_path.is_file() else "null"
            merge_metadata(
                str(output / "metadata.json"),
                item.experiment,
                "rpi5",
                utc_now(),
                started_at,
                str(finished_ns - started_ns),
                item.config,
                config_sha,
                json.dumps(
                    {
                        "profile_path": item.loadgen_profile,
                        "warmup_secs": item.warmup_secs,
                        "offered_rate_msg_s": item.offered_rate_msg_s,
                    }
                ),
                json.dumps({"broker": "127.0.0.1:1883", "managed_by_harness": False}),
                str(runtime_exit),
                provenance,
            )
        postprocess_run(root, item, output)
        result = write_rate_sweep_result(root, item, output, measurement_duration_ns)
        if result["throttled"]:
            raise RuntimeError("Pi throttling occurred during rate-sweep measurement")
        verify_result(root, output)
    except (
        OSError,
        ValueError,
        RuntimeError,
        subprocess.CalledProcessError,
        subprocess.TimeoutExpired,
    ) as error:
        for process in (publisher, subscriber, runtime):
            if process is not None and process.poll() is None:
                process.kill()
                process.wait()
        if sampler is not None:
            sampler.stop()
        if ekuiper_active:
            set_ekuiper_active(root, False)
        stop_pi_telemetry(telemetry)
        write_status(output, item, "failed", str(error))
        print(f"[{utc_now()}] FAIL {item.result_key}: {error}", flush=True)
        return False
    write_status(output, item, "passed")
    print(f"[{utc_now()}] PASS {item.result_key}", flush=True)
    return True


def run_item(root: Path, batch_id: str, item: RunItem) -> bool:
    condition_dir = (
        root
        / "eval/results"
        / item.experiment
        / f"rpi5-{batch_id}"
        / item.condition
    )
    selection = select_attempt(condition_dir, item.run_index)
    if selection.skip:
        print(f"[{utc_now()}] SKIP {item.result_key}: {selection.path}", flush=True)
        return True

    if item.shared_from:
        try:
            result = copy_shared_result(root, batch_id, item)
            verify_result(root, result)
            write_status(result, item, "passed", f"shared from {item.shared_from}")
            print(f"[{utc_now()}] PASS {item.result_key} (shared)", flush=True)
            return True
        except (OSError, ValueError, RuntimeError, subprocess.CalledProcessError) as error:
            write_status(selection.path, item, "failed", str(error))
            print(f"[{utc_now()}] FAIL {item.result_key}: {error}", flush=True)
            return False

    if item.experiment == "e-perf-10":
        return run_rate_sweep_item(root, item, selection)
    if item.experiment in {"e-swap-1", "e-swap-4", "e-swap-5"}:
        return run_hot_swap_item(root, item, selection)
    if item.experiment == "e-swap-3" and item.condition != "wafer-hotswap":
        return run_restart_item(root, item, selection)
    if item.system == "ekuiper":
        return run_ekuiper_item(root, item, selection)

    output = selection.path
    set_ekuiper_active(root, False)
    command = [
        str(root / "eval/scripts/run-experiment.sh"),
        "--config",
        str(root / item.config),
        "--experiment",
        item.experiment,
        "--host",
        "rpi5",
        "--canonical",
        "--defer-verification",
        "--skip-build",
        "--broker",
        "127.0.0.1:1883",
        "--duration",
        str(max(30, item.warmup_secs + item.measurement_secs + 30)),
        "--output-dir",
        str(output),
    ]
    if item.loadgen_profile:
        command.extend(
            [
                "--loadgen-profile",
                str(root / item.loadgen_profile),
                "--warmup-secs",
                str(item.warmup_secs),
            ]
        )
    if item.total_messages is not None:
        command.extend(["--total-messages", str(item.total_messages)])
    if item.startup_mode is not None:
        command.extend(["--startup-cache-state", item.startup_mode])

    env = os.environ.copy()
    env["WAFER_RUNTIME_CPUSET"] = item.runtime_cpus
    env["WAFER_LOADGEN_CPUSET"] = item.support_cpus
    print(f"[{utc_now()}] START {item.result_key} -> {output}", flush=True)
    try:
        subprocess.run(command, cwd=root, env=env, check=True)
        postprocess_run(root, item, output)
        verify_result(root, output)
    except (OSError, ValueError, subprocess.CalledProcessError) as error:
        write_status(output, item, "failed", str(error))
        print(f"[{utc_now()}] FAIL {item.result_key}: {error}", flush=True)
        return False
    write_status(output, item, "passed")
    print(f"[{utc_now()}] PASS {item.result_key}", flush=True)
    return True


def postprocess_run(root: Path, item: RunItem, output: Path) -> None:
    metadata_path = output / "metadata.json"
    metadata = json.loads(metadata_path.read_text())
    metadata["system"] = item.system
    metadata["condition"] = item.condition
    metadata["run_index"] = item.run_index
    metadata["config_path"] = item.config
    if item.experiment == "e-perf-10":
        metadata["thesis_evidence"] = False
        metadata["offered_rate_msg_s"] = item.offered_rate_msg_s
    if item.experiment in HOTSWAP_SHARED_EXPERIMENTS:
        metadata["measurement_source_leaf"] = str(output.relative_to(root))
        metadata["shared_measurement"] = False
    stamp_focused_metadata(root, metadata)
    if item.loadgen_profile:
        profile = root / item.loadgen_profile
        loadgen = metadata.get("loadgen") or {}
        loadgen["profile_path"] = item.loadgen_profile
        loadgen["profile_sha256"] = hashlib.sha256(profile.read_bytes()).hexdigest()
        loadgen["warmup_secs"] = item.warmup_secs
        metadata["loadgen"] = loadgen
    boundary_path = output / "power-boundary.json"
    if boundary_path.is_file():
        boundary = json.loads(boundary_path.read_text())
        telemetry_path = output / "pi-telemetry.csv"
        boundary["valid"] = (
            telemetry_path.is_file()
            and telemetry_path.stat().st_size > 100
            and not (output / "telemetry-error.json").exists()
        )
        metadata["power_measurement"] = boundary
    if item.experiment == "e-iso-7":
        metadata["branch_measurement"] = {
            "boundary": "branch_sink_post_warmup",
            "warmup_secs": item.warmup_secs,
            "measurement_secs": item.measurement_secs,
            "artifact_directories": {"branch_a": "branch-a", "branch_b": "branch-b"},
        }
    metadata_path.write_text(json.dumps(metadata, indent=2) + "\n")

    subscriber_metadata = output / "subscriber-metadata.json"
    if subscriber_metadata.is_file():
        subprocess.run(
            [
                sys.executable,
                str(root / "eval/scripts/lib/write_throughput.py"),
                str(subscriber_metadata),
                str(output / "throughput.csv"),
            ],
            check=True,
        )

    loadgen = root / "target/release/wafer-loadgen"
    if item.experiment == "e-iso-7":
        for branch_dir in ("branch-a", "branch-b"):
            branch = output / branch_dir
            subprocess.run(
                [
                    str(loadgen),
                    "hdr-summary",
                    "--hdr",
                    str(branch / "latency.hdr"),
                    "--output",
                    str(branch / "percentiles.json"),
                ],
                check=True,
            )
        config = tomllib.loads((root / item.config).read_text())
        source = next(
            node
            for node in config["nodes"].values()
            if node.get("kind") == "bench-source"
        )
        branch_isolation = derive_branch_isolation(
            output,
            warmup_secs=item.warmup_secs,
            measurement_secs=item.measurement_secs,
            offered_messages=int(source["total_messages"]),
        )
        branch_isolation["condition"] = item.condition
        branch_isolation["run_index"] = item.run_index
        (output / "branch-isolation.json").write_text(
            json.dumps(branch_isolation, indent=2) + "\n"
        )
        branch_a_window = output / "branch-a/measurement-window.json"
        if branch_a_window.is_file():
            shutil.copyfile(branch_a_window, output / "measurement-window.json")

    if item.experiment in {"e-swap-1", "e-swap-4"}:
        requests = json.loads((output / "swap_requests.json").read_text())
        sink_timeline = json.loads((output / "swap_timeline.json").read_text())
        evidence = derive_hotswap_evidence(
            requests,
            sink_timeline,
            experiment=item.experiment,
            condition=item.condition,
            source_leaf=str(output.relative_to(root)),
        )
        (output / "hotswap-analysis.json").write_text(json.dumps(evidence, indent=2) + "\n")

    if item.experiment == "e-backpressure":
        definition = json.loads((root / "eval/canonical-matrix.json").read_text())["experiments"][
            "e-backpressure"
        ]
        config = tomllib.loads((root / item.config).read_text())
        source = next(
            node for node in config["nodes"].values() if node.get("kind") == "bench-source"
        )
        offered_messages = int(source["total_messages"])
        offered_duration_ns = int(offered_messages / float(source["rate"]) * 1_000_000_000)
        summary = analyze_backpressure(
            read_queue_depth(output / "queue-depth.csv"),
            offered_messages=offered_messages,
            offered_duration_ns=offered_duration_ns,
            occupancy_threshold=float(definition["queue_occupancy_threshold"]),
            recovery_threshold=float(definition["queue_recovery_threshold"]),
        )
        with (output / "memory.csv").open(newline="") as handle:
            rss_peak = max(int(row["rss_bytes"]) for row in csv.DictReader(handle))
        with (output / "sequence.csv").open(newline="") as handle:
            sequence = next(csv.DictReader(handle))
        received = int(sequence["total_received"])
        gaps = int(sequence["gap_msgs"])
        duplicates = int(sequence["duplicates_count"])
        summary["sequence"] = {
            "offered": offered_messages,
            "received": received,
            "gaps": gaps,
            "duplicates": duplicates,
            "lossless": received == offered_messages and gaps == 0 and duplicates == 0,
        }
        summary["memory"] = {
            "peak_rss_bytes": rss_peak,
            "limit_bytes": int(definition["rss_limit_bytes"]),
            "within_limit": rss_peak <= int(definition["rss_limit_bytes"]),
        }
        (output / "backpressure.json").write_text(json.dumps(summary, indent=2) + "\n")
        validate_backpressure_result(summary)

    if item.experiment.startswith("e-iso-"):
        containment = derive_containment(output)
        containment["experiment"] = item.experiment
        containment["condition"] = item.condition
        (output / "containment.json").write_text(json.dumps(containment, indent=2) + "\n")
        if item.experiment == "e-iso-8":
            recovery = summarize_recovery(output / "recovery.csv")
            (output / "recovery.json").write_text(json.dumps(recovery, indent=2) + "\n")

    hdr = output / "latency.hdr"
    subscriber_metadata = output / "subscriber-metadata.json"
    if subscriber_metadata.is_file():
        subscriber = json.loads(subscriber_metadata.read_text())
        percentiles = {
            "total_count": subscriber.get("total_recorded", 0),
            "p50_ns": subscriber.get("latency_p50_ns", 0),
            "p95_ns": subscriber.get("latency_p95_ns", 0),
            "p99_ns": subscriber.get("latency_p99_ns", 0),
            "p999_ns": subscriber.get("latency_p999_ns", 0),
        }
        (output / "percentiles.json").write_text(json.dumps(percentiles, indent=2) + "\n")
    elif hdr.is_file():
        subprocess.run(
            [str(loadgen), "hdr-summary", "--hdr", str(hdr), "--output", str(output / "percentiles.json")],
            check=True,
        )
    if item.experiment == "e-perf-9":
        startup_path = output / "startup.json"
        startup = json.loads(startup_path.read_text())
        validate_startup_artifact(startup)
        if startup["cache_state"] != item.startup_mode:
            raise ValueError(
                f"startup cache state {startup['cache_state']!r} does not match "
                f"condition {item.startup_mode!r}"
            )


def verify_result(root: Path, output: Path) -> None:
    subprocess.run(
        [
            sys.executable,
            str(root / "eval/scripts/verify-result-contract.py"),
            "--canonical",
            str(output),
        ],
        cwd=root,
        check=True,
    )


def verify_focused_batch(root: Path, batch_id: str, experiments: set[str]) -> None:
    result_dirs = [
        root / "eval/results" / experiment / f"rpi5-{batch_id}"
        for experiment in sorted(experiments)
    ]
    subprocess.run(
        [
            sys.executable,
            str(root / "eval/scripts/verify-result-contract.py"),
            "--canonical",
            "--focused",
            *map(str, result_dirs),
        ],
        cwd=root,
        check=True,
    )


def summarize_branch_isolation(root: Path, batch_id: str) -> Path:
    result_root = root / "eval/results/e-iso-7" / f"rpi5-{batch_id}"
    runs: dict[str, list[dict]] = {
        "control": [],
        "panic-attack": [],
        "epoch-loop-attack": [],
    }
    for path in result_root.rglob("branch-isolation.json"):
        status_path = path.parent / "canonical-status.json"
        try:
            if json.loads(status_path.read_text()).get("status") != "passed":
                continue
            result = json.loads(path.read_text())
            runs[result["condition"]].append(result)
        except (KeyError, OSError, TypeError, ValueError):
            continue

    summary = {
        "schema_version": 1,
        "experiment": "e-iso-7",
        "batch_id": batch_id,
        "sample_unit": "run",
        "comparisons": compare_branch_conditions(runs),
    }
    path = root / "eval/results/canonical-batches" / f"rpi5-{batch_id}" / "branch-isolation-summary.json"
    path.write_text(json.dumps(summary, indent=2) + "\n")
    return path


def summarize_rate_sweep(root: Path, batch_id: str) -> Path:
    result_root = root / "eval/results/e-perf-10" / f"rpi5-{batch_id}"
    by_system: dict[str, list[dict]] = {system: [] for system in RATE_SWEEP_SYSTEMS}
    for path in result_root.rglob("rate-sweep.json"):
        status_path = path.parent / "canonical-status.json"
        try:
            if json.loads(status_path.read_text()).get("status") != "passed":
                continue
            result = json.loads(path.read_text())
            validate_rate_sweep_result(result)
        except (OSError, ValueError, KeyError, TypeError):
            continue
        messages = result["messages"]
        by_system[result["system"]].append(
            {
                "offered_rate_msg_s": result["offered_rate_msg_s"],
                "p99_ns": result["latency_ns"]["p99"],
                "offered": messages["offered"],
                "lost": messages["lost"],
            }
        )

    definition = json.loads((root / "eval/canonical-matrix.json").read_text())["experiments"]["e-perf-10"]
    expected_repetitions = int(definition["repetitions"])
    systems = {}
    for system, samples in by_system.items():
        observed = Counter(sample["offered_rate_msg_s"] for sample in samples)
        complete = all(
            observed[rate] == expected_repetitions for rate in RATE_SWEEP_RATES
        )
        if RATE_SWEEP_BASELINE not in observed:
            systems[system] = {
                "complete": False,
                "error": f"missing {RATE_SWEEP_BASELINE} msg/s baseline",
                "observed_samples": dict(sorted(observed.items())),
            }
            continue
        systems[system] = {
            "complete": complete,
            "observed_samples": dict(sorted(observed.items())),
            **classify_sustainable_throughput(samples),
        }

    summary = {
        "schema_version": 1,
        "experiment": "e-perf-10",
        "batch_id": batch_id,
        "thesis_evidence": False,
        "criteria": {
            "baseline_rate_msg_s": RATE_SWEEP_BASELINE,
            "p99_multiplier_limit": RATE_SWEEP_P99_MULTIPLIER,
            "max_loss_percent": RATE_SWEEP_MAX_LOSS_PERCENT,
        },
        "systems": systems,
    }
    path = root / "eval/results/canonical-batches" / f"rpi5-{batch_id}" / "rate-sweep-summary.json"
    path.write_text(json.dumps(summary, indent=2) + "\n")
    return path


def summarise(root: Path, batch_id: str, experiments: set[str]) -> None:
    if "e-perf-10" in experiments:
        summarize_rate_sweep(root, batch_id)
    if "e-iso-7" in experiments:
        summarize_branch_isolation(root, batch_id)
    scripts = {
        "e-perf-4": "summarise-e-perf-4.sh",
        "e-perf-6": "summarise-e-perf-6-8.sh",
        "e-perf-8": "summarise-e-perf-6-8.sh",
        "e-perf-7": "summarise-e-perf-7.sh",
    }
    for experiment in EXPERIMENT_ORDER:
        script = scripts.get(experiment)
        if experiment not in experiments or script is None:
            continue
        result_root = root / "eval/results" / experiment / f"rpi5-{batch_id}"
        subprocess.run([str(root / "eval/scripts" / script), str(result_root)], check=True)


def validate_focused_freeze(root: Path, matrix_path: Path) -> dict:
    receipt_path = root / "eval/focused-pilot-freeze.json"
    receipt = json.loads(receipt_path.read_text())
    matrix_sha256 = hashlib.sha256(matrix_path.read_bytes()).hexdigest()
    if receipt.get("status") != "frozen-before-execution":
        raise ValueError("focused-pilot freeze receipt is not frozen-before-execution")
    if receipt.get("canonical_matrix_sha256") != matrix_sha256:
        raise ValueError("focused-pilot matrix changed after freeze")
    schedule_path = root / str(receipt.get("schedule_path", ""))
    if receipt.get("schedule_sha256") != hashlib.sha256(schedule_path.read_bytes()).hexdigest():
        raise ValueError("focused-pilot schedule changed after freeze")
    if receipt.get("thesis_evidence") is not False:
        raise ValueError("focused-pilot freeze receipt must set thesis_evidence=false")
    return receipt


def print_plan(schedule: list[RunItem], seed: int, batch_id: str) -> None:
    print(f"batch_id={batch_id} seed={seed} host=rpi5")
    seen: set[tuple[str, str]] = set()
    for item in schedule:
        key = (item.experiment, item.condition)
        if key in seen:
            continue
        seen.add(key)
        suffix = f" shared_from={item.shared_from}" if item.shared_from else ""
        print(
            f"PLAN {item.experiment} condition={item.condition} runs="
            f"{sum(1 for candidate in schedule if candidate.experiment == item.experiment and candidate.condition == item.condition)} "
            f"warmup={item.warmup_secs}s measurement={item.measurement_secs}s "
            f"sut_cpus={item.runtime_cpus} support_cpus={item.support_cpus}{suffix}"
        )


def parse_experiments(raw: str) -> set[str]:
    if raw == "all":
        return set(CONDITIONS)
    values = {value.strip() for value in raw.split(",") if value.strip()}
    if not values:
        raise ValueError("--experiments must not be empty")
    return values


def main() -> int:
    parser = argparse.ArgumentParser(description="Run resumable canonical Pi 5 evaluations")
    parser.add_argument("--root", type=Path, default=Path(__file__).resolve().parents[3])
    parser.add_argument("--experiments", default="all")
    parser.add_argument("--focused", action="store_true")
    parser.add_argument("--batch-id")
    parser.add_argument("--seed", type=int, default=1729)
    parser.add_argument("--dry-run", action="store_true")
    parser.add_argument("--execute", action="store_true")
    args = parser.parse_args()
    if args.dry_run == args.execute:
        parser.error("choose exactly one of --dry-run or --execute")

    root = args.root.resolve()
    try:
        focused_freeze = None
        if args.focused:
            if args.experiments != "all":
                raise ValueError("--focused cannot be combined with --experiments")
            matrix_path = root / "eval/canonical-matrix.json"
            focused_freeze = validate_focused_freeze(root, matrix_path)
            schedule = build_focused_schedule(args.seed)
            frozen_schedule = json.loads(
                (root / focused_freeze["schedule_path"]).read_text()
            )
            if frozen_schedule != [item.__dict__ for item in schedule]:
                raise ValueError("focused-pilot runner schedule differs from frozen snapshot")
            experiments = {item.experiment for item in schedule}
        else:
            experiments = parse_experiments(args.experiments)
            schedule = build_schedule(experiments, args.seed)
    except (KeyError, OSError, TypeError, ValueError) as error:
        parser.error(str(error))

    batch_id = args.batch_id or dt.datetime.now(dt.timezone.utc).strftime("%Y-%m-%dT%H-%M-%SZ")
    if not all(character.isalnum() or character in "._-" for character in batch_id):
        parser.error("--batch-id contains unsafe characters")
    print_plan(schedule, args.seed, batch_id)
    if args.dry_run:
        return 0

    if focused_freeze is not None:
        os.environ[FOCUSED_MATRIX_SHA_ENV] = focused_freeze["canonical_matrix_sha256"]

    ledger = root / "eval/results/canonical-batches" / f"rpi5-{batch_id}"
    ledger.mkdir(parents=True, exist_ok=True)
    schedule_json = json.dumps([item.__dict__ for item in schedule], indent=2) + "\n"
    (ledger / "schedule.json").write_text(schedule_json)
    if focused_freeze is not None:
        (ledger / "focused-pilot-execution.json").write_text(
            json.dumps(
                {
                    "schema_version": 1,
                    "batch_id": batch_id,
                    "thesis_evidence": False,
                    "source_git_sha": subprocess.check_output(
                        ["git", "-C", str(root), "rev-parse", "HEAD"], text=True
                    ).strip(),
                    "canonical_matrix_sha256": focused_freeze["canonical_matrix_sha256"],
                    "freeze_receipt_sha256": hashlib.sha256(
                        (root / "eval/focused-pilot-freeze.json").read_bytes()
                    ).hexdigest(),
                    "schedule_sha256": hashlib.sha256(schedule_json.encode()).hexdigest(),
                    "started_at": utc_now(),
                },
                indent=2,
            )
            + "\n"
        )

    failures: list[str] = []
    completed = 0
    total = len(schedule)
    validation_items = [item for item in schedule if item.experiment == "e-val-1"]
    remaining_items = [item for item in schedule if item.experiment != "e-val-1"]
    write_progress(ledger, "batch-started", completed, total)
    for item in validation_items:
        write_progress(ledger, "item-started", completed, total, item.result_key, len(failures))
        if not run_item(root, batch_id, item):
            failures.append(item.result_key)
        completed += 1
        write_progress(ledger, "item-finished", completed, total, item.result_key, len(failures))

    if validation_items:
        validation_root = root / "eval/results/e-val-1" / f"rpi5-{batch_id}" / "delay-50ms"
        gate = evaluate_validation_gate(validation_root, expected_runs=30)
        (ledger / "e-val-1-gate.json").write_text(
            json.dumps(gate.__dict__, indent=2) + "\n"
        )
        if not gate.passed:
            print(f"[{utc_now()}] STOP E-Val-1 gate failed: {gate.failed_runs}", flush=True)
            write_progress(ledger, "batch-stopped", completed, total, failures=len(failures))
            return 1

    for item in remaining_items:
        write_progress(ledger, "item-started", completed, total, item.result_key, len(failures))
        if not run_item(root, batch_id, item):
            failures.append(item.result_key)
        completed += 1
        write_progress(ledger, "item-finished", completed, total, item.result_key, len(failures))
    summarise(root, batch_id, experiments)
    if focused_freeze is not None and not failures:
        verify_focused_batch(root, batch_id, experiments)
    (ledger / "failures.json").write_text(json.dumps(failures, indent=2) + "\n")
    write_progress(ledger, "batch-finished", completed, total, failures=len(failures))
    return 1 if failures else 0


if __name__ == "__main__":
    raise SystemExit(main())
