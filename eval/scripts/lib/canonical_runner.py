from __future__ import annotations

import argparse
import csv
import datetime as dt
import hashlib
import json
import math
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
CANONICAL_MATRIX_PATH = Path(__file__).resolve().parents[2] / "canonical-matrix.json"
RATE_SWEEP_DEFINITION = json.loads(CANONICAL_MATRIX_PATH.read_text())["experiments"]["e-perf-10"]
RATE_SWEEP_RATES = tuple(RATE_SWEEP_DEFINITION["rate_points_msg_s"])
RATE_SWEEP_REPETITIONS = int(RATE_SWEEP_DEFINITION["repetitions"])
RATE_SWEEP_WARMUP_SECS = int(RATE_SWEEP_DEFINITION["warmup_secs"])
RATE_SWEEP_MEASUREMENT_SECS = int(RATE_SWEEP_DEFINITION["measurement_secs"])
HISTORICAL_RATE_SWEEP_RATES = (500, 1_000, 2_000, 4_000, 8_000, 16_000)
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
CAPACITY_SCOUT_SYSTEMS = RATE_SWEEP_SYSTEMS
CAPACITY_SCOUT_SUTS = ("native", "wafer", "ekuiper")
CAPACITY_SCOUT_BASE_RATE = 500
CAPACITY_SCOUT_REPETITIONS = 3
CAPACITY_SCOUT_WARMUP_SECS = 30
CAPACITY_SCOUT_MEASUREMENT_SECS = 60
CAPACITY_SCOUT_SEED = 1729
CAPACITY_SCOUT_WAFER_CONFIG = "eval/configs/capacity-scout-wafer.toml"
CAPACITY_SCOUT_ATTEMPT_TIMEOUT_SECS = 240
CAPACITY_SCOUT_BATCH_TIMEOUT_SECS = 18 * 60 * 60
CAPACITY_SCOUT_DISK_FLOOR_BYTES = 2 * 1024 * 1024 * 1024
SWAP3_STRATEGIES = ("wafer-hotswap", "wafer-restart", "ekuiper-restart")
SWAP3_EVENT_OFFSET_NS = 60_000_000_000
SWAP3_BUCKET_WIDTH_NS = 100_000_000
SWAP3_COVERAGE_START_NS = -10_000_000_000
SWAP3_COVERAGE_END_NS = 10_000_000_000
SWAP3_ALIGNMENT_TOLERANCE_NS = 10_000_000


def analyze_capacity_scout_summary(
    publisher: dict, subscriber: dict, measurement_duration_ns: int
) -> dict:
    if measurement_duration_ns <= 0:
        raise ValueError("capacity-scout measurement duration must be positive")
    try:
        intended = int(publisher["intended"])
        rejected = int(publisher["rejected"])
        enqueued = int(publisher["enqueued"])
        received_events = int(subscriber["total_recorded"])
        duplicates = int(subscriber["sequence"]["total_duplicates"])
        ignored_warmup = int(subscriber.get("ignored_sequence_count", 0))
        unexpected = int(subscriber["unexpected_sequence_count"])
    except (KeyError, TypeError, ValueError) as error:
        raise ValueError("capacity-scout counters are missing or invalid") from error
    if min(intended, rejected, enqueued, received_events, duplicates, ignored_warmup, unexpected) < 0:
        raise ValueError("capacity-scout counters must be non-negative")
    if intended != rejected + enqueued:
        raise ValueError("counter identity failed: intended = rejected + enqueued")
    if subscriber.get("parse_errors") != 0 or subscriber.get("negative_latency_count") != 0:
        raise ValueError("subscriber reported parse errors or negative latency")
    if int(subscriber.get("sequence", {}).get("total_received", -1)) != received_events:
        raise ValueError("subscriber received counters do not reconcile")
    if int(subscriber.get("total_messages", received_events)) != received_events:
        raise ValueError("subscriber message and HDR populations differ")
    if duplicates > received_events:
        raise ValueError("duplicates exceed received events")
    received_unique = received_events - duplicates
    if received_unique > enqueued:
        raise ValueError("unique receipts exceed enqueued messages")
    downstream_lost = enqueued - received_unique
    total_undelivered = rejected + downstream_lost
    duration_secs = measurement_duration_ns / 1_000_000_000
    return {
        "messages": {
            "intended": intended,
            "rejected": rejected,
            "enqueued": enqueued,
            "received_events": received_events,
            "received_unique": received_unique,
            "downstream_lost": downstream_lost,
            "total_undelivered": total_undelivered,
            "duplicates": duplicates,
            "unexpected": unexpected,
            "ignored_warmup": ignored_warmup,
        },
        "rates_msg_s": {
            "intended": intended / duration_secs,
            "achieved": received_unique / duration_secs,
        },
        "loss_percent": 100.0 * total_undelivered / intended if intended else 100.0,
        "latency_ns": {
            "p50": int(subscriber["latency_p50_ns"]),
            "p95": int(subscriber["latency_p95_ns"]),
            "p99": int(subscriber["latency_p99_ns"]),
        },
    }


def validate_capacity_scout_result(result: dict) -> None:
    required = {
        "schema_version", "batch_class", "thesis_evidence", "system", "rate_msg_s",
        "source_git_sha", "source_dirty", "measurement_duration_ns", "messages", "rates_msg_s",
        "loss_percent", "latency_hdr", "resources", "thermal", "process_audit",
        "config", "loadgen_profile", "provenance", "controlled_factors", "traces",
    }
    missing = sorted(required - result.keys())
    if missing:
        raise ValueError(f"capacity-scout result missing fields: {', '.join(missing)}")
    if result["batch_class"] != "capacity-scout":
        raise ValueError("capacity-scout result requires batch_class=capacity-scout")
    if result["thesis_evidence"] is not False:
        raise ValueError("capacity-scout result requires thesis_evidence=false")
    if result["traces"] is not False:
        raise ValueError("capacity-scout result must not contain per-message traces")
    if result["system"] not in CAPACITY_SCOUT_SYSTEMS:
        raise ValueError("capacity-scout result has an unknown system")
    controlled_fields = {
        "broker", "topic", "payload_template_sha256", "qos", "warmup_secs",
        "measurement_secs", "load_shape", "sequence_example_limit", "support_cpus", "sut_cpus",
    }
    if set(result["controlled_factors"]) != controlled_fields:
        raise ValueError("capacity-scout controlled factors are incomplete")
    if type(result["rate_msg_s"]) is not int or result["rate_msg_s"] <= 0:
        raise ValueError("capacity-scout rate must be a positive integer")
    if not re.fullmatch(r"[0-9a-f]{40}", str(result["source_git_sha"])):
        raise ValueError("capacity-scout source_git_sha is invalid")
    if result["source_dirty"] is not False:
        raise ValueError("capacity-scout source must be clean")
    if set(result["resources"]) != {"scope", "cpu_percent", "max_rss_bytes"}:
        raise ValueError("capacity-scout resource summary is incomplete")
    if result["resources"]["scope"] != (
        "no-sut" if result["system"] == "mqtt-loopback" else "sut"
    ):
        raise ValueError("capacity-scout resource scope differs from system")
    if set(result["thermal"]) != {"max_temperature_millicelsius", "throttled"}:
        raise ValueError("capacity-scout thermal summary is incomplete")
    summary = analyze_capacity_scout_summary(
        {field: result["messages"][field] for field in ("intended", "rejected", "enqueued")},
        {
            "total_recorded": result["messages"]["received_events"],
            "total_messages": result["messages"]["received_events"],
            "ignored_sequence_count": result["messages"]["ignored_warmup"],
            "unexpected_sequence_count": result["messages"]["unexpected"],
            "parse_errors": 0,
            "negative_latency_count": 0,
            "latency_p50_ns": 0,
            "latency_p95_ns": 0,
            "latency_p99_ns": 0,
            "sequence": {
                "total_received": result["messages"]["received_events"],
                "total_duplicates": result["messages"]["duplicates"],
            },
        },
        int(result["measurement_duration_ns"]),
    )
    for field, value in summary["messages"].items():
        if result["messages"].get(field) != value:
            raise ValueError(f"capacity-scout message counter differs: {field}")
    for field, value in summary["rates_msg_s"].items():
        if not math.isclose(float(result["rates_msg_s"].get(field, -1)), value):
            raise ValueError(f"capacity-scout rate differs: {field}")
    if not math.isclose(float(result["loss_percent"]), summary["loss_percent"]):
        raise ValueError("capacity-scout loss_percent differs from counters")
    if int(result["latency_hdr"].get("samples", -1)) != result["messages"]["received_events"]:
        raise ValueError("latency HDR count differs from received events")
    if result["thermal"].get("throttled") is not False:
        raise ValueError("capacity-scout result is throttled")
    if int(result["thermal"].get("max_temperature_millicelsius", 0)) >= 75_000:
        raise ValueError("capacity-scout result reached the 75 C thermal stop")
    if result["messages"]["unexpected"] != 0:
        raise ValueError("capacity-scout result contains unexpected sequences")
    for receipt in (
        "latency_hdr", "process_audit", "config", "loadgen_profile", "provenance"
    ):
        if not re.fullmatch(r"[0-9a-f]{64}", str(result[receipt].get("sha256", ""))):
            raise ValueError(f"capacity-scout {receipt} receipt has invalid sha256")


def verify_capacity_scout_result_files(output: Path, result: dict) -> None:
    validate_capacity_scout_result(result)
    for name in ("latency_hdr", "process_audit", "config", "loadgen_profile", "provenance"):
        receipt = result[name]
        path = output / receipt["path"]
        if not path.is_file():
            raise ValueError(f"capacity-scout {name} receipt path is missing")
        if hashlib.sha256(path.read_bytes()).hexdigest() != receipt["sha256"]:
            raise ValueError(f"capacity-scout {name} receipt checksum mismatch")


def validate_capacity_run_result(result: dict) -> None:
    required = {
        "schema_version", "batch_class", "experiment", "thesis_evidence", "system",
        "rate_msg_s", "run_index", "source_git_sha", "source_dirty", "measurement_duration_ns",
        "messages", "rates_msg_s", "loss_percent", "latency_ns", "latency_hdr",
        "resources", "thermal", "process_audit", "config", "loadgen_profile",
        "provenance", "controlled_factors", "traces",
    }
    missing = sorted(required - result.keys())
    if missing:
        raise ValueError(f"capacity-run result missing fields: {', '.join(missing)}")
    if result["batch_class"] != "final-capacity":
        raise ValueError("capacity-run result requires batch_class=final-capacity")
    if result["experiment"] != "e-perf-10" or result["thesis_evidence"] is not True:
        raise ValueError("capacity-run result must be final E-Perf-10 evidence")
    if result["traces"] is not False:
        raise ValueError("capacity-run result must not contain per-message traces")
    if result["system"] not in RATE_SWEEP_SYSTEMS:
        raise ValueError("capacity-run result has an unknown system")
    controlled_fields = {
        "broker", "topic", "payload_template_sha256", "qos", "warmup_secs",
        "measurement_secs", "load_shape", "sequence_example_limit", "support_cpus", "sut_cpus",
    }
    if set(result["controlled_factors"]) != controlled_fields:
        raise ValueError("capacity-run controlled factors are incomplete")
    if result["rate_msg_s"] not in RATE_SWEEP_RATES:
        raise ValueError("capacity-run result rate is outside the frozen grid")
    run_index = result.get("run_index")
    if type(run_index) is not int or not 1 <= run_index <= RATE_SWEEP_REPETITIONS:
        raise ValueError("capacity-run run_index is outside the frozen repetitions")
    controlled = result["controlled_factors"]
    if controlled.get("warmup_secs") != RATE_SWEEP_WARMUP_SECS:
        raise ValueError("capacity-run warmup differs from the frozen matrix")
    if controlled.get("measurement_secs") != RATE_SWEEP_MEASUREMENT_SECS:
        raise ValueError("capacity-run controlled measurement duration differs from the frozen matrix")
    expected_duration_ns = RATE_SWEEP_MEASUREMENT_SECS * 1_000_000_000
    if result["measurement_duration_ns"] != expected_duration_ns:
        raise ValueError("capacity-run measurement duration differs from the frozen matrix")
    if result["messages"].get("intended") != result["rate_msg_s"] * RATE_SWEEP_MEASUREMENT_SECS:
        raise ValueError("capacity-run intended population differs from rate times duration")
    if not re.fullmatch(r"[0-9a-f]{40}", str(result["source_git_sha"])):
        raise ValueError("capacity-run source_git_sha is invalid")
    if result["source_dirty"] is not False:
        raise ValueError("capacity-run source must be clean")
    summary = analyze_capacity_scout_summary(
        {field: result["messages"][field] for field in ("intended", "rejected", "enqueued")},
        {
            "total_recorded": result["messages"]["received_events"],
            "total_messages": result["messages"]["received_events"],
            "ignored_sequence_count": result["messages"].get("ignored_warmup", 0),
            "unexpected_sequence_count": result["messages"]["unexpected"],
            "parse_errors": 0,
            "negative_latency_count": 0,
            **{f"latency_{name}_ns": result["latency_ns"][name] for name in ("p50", "p95", "p99")},
            "sequence": {
                "total_received": result["messages"]["received_events"],
                "total_duplicates": result["messages"]["duplicates"],
            },
        },
        int(result["measurement_duration_ns"]),
    )
    if summary["messages"] != result["messages"]:
        raise ValueError("capacity-run message counters do not reconcile")
    for field in ("intended", "achieved"):
        if not math.isclose(float(result["rates_msg_s"].get(field, -1)), summary["rates_msg_s"][field]):
            raise ValueError(f"capacity-run {field} rate differs from counters")
    achieved_ratio = summary["rates_msg_s"]["achieved"] / result["rate_msg_s"]
    if not math.isclose(float(result["rates_msg_s"].get("achieved_ratio", -1)), achieved_ratio):
        raise ValueError("capacity-run achieved ratio differs from counters")
    if not math.isclose(float(result["loss_percent"]), summary["loss_percent"]):
        raise ValueError("capacity-run loss_percent differs from counters")
    if result["latency_ns"] != summary["latency_ns"]:
        raise ValueError("capacity-run latency summary differs from subscriber metadata")
    histogram = result["latency_hdr"]
    if int(histogram.get("samples", -1)) != result["messages"]["received_events"]:
        raise ValueError("capacity-run HDR count differs from received events")
    if {
        "lowest_ns": histogram.get("lowest_ns"),
        "highest_ns": histogram.get("highest_ns"),
        "significant_digits": histogram.get("significant_digits"),
    } != {"lowest_ns": 1_000, "highest_ns": 10_000_000_000, "significant_digits": 3}:
        raise ValueError("capacity-run HDR precision differs from the frozen recorder")
    if result["thermal"].get("throttled") is not False:
        raise ValueError("capacity-run result is throttled")
    if result["messages"]["unexpected"] != 0:
        raise ValueError("capacity-run result contains unexpected sequences")
    for receipt in ("latency_hdr", "process_audit", "config", "loadgen_profile", "provenance"):
        if not re.fullmatch(r"[0-9a-f]{64}", str(result[receipt].get("sha256", ""))):
            raise ValueError(f"capacity-run {receipt} receipt has invalid sha256")


def verify_capacity_result_files(output: Path, result: dict) -> None:
    validate_capacity_run_result(result)
    expected_paths = {
        "latency_hdr": "latency.hdr",
        "process_audit": "process-audit.json",
        "config": "config.toml",
        "loadgen_profile": "loadgen-profile.toml",
        "provenance": "metadata.json",
    }
    for name, expected_path in expected_paths.items():
        receipt = result[name]
        if receipt.get("path") != expected_path:
            raise ValueError(f"capacity-run {name} receipt path is invalid")
        path = output / expected_path
        if not path.is_file():
            raise ValueError(f"capacity-run {name} receipt path is missing")
        if hashlib.sha256(path.read_bytes()).hexdigest() != receipt["sha256"]:
            raise ValueError(f"capacity-run {name} receipt checksum mismatch")


def _percentile(values: list[float], fraction: float) -> float:
    ordered = sorted(values)
    if not ordered:
        raise ValueError("cannot summarize an empty sample")
    position = (len(ordered) - 1) * fraction
    lower = math.floor(position)
    upper = math.ceil(position)
    if lower == upper:
        return ordered[lower]
    return ordered[lower] + (ordered[upper] - ordered[lower]) * (position - lower)


def _bootstrap_median_ci(values: list[float], seed: str) -> list[float]:
    generator = random.Random(int(hashlib.sha256(seed.encode()).hexdigest(), 16))
    medians = [
        statistics.median(generator.choices(values, k=len(values)))
        for _ in range(2_000)
    ]
    return [_percentile(medians, 0.025), _percentile(medians, 0.975)]


def _run_summary(values: list[float], seed: str) -> dict:
    return {
        "min": min(values),
        "median": statistics.median(values),
        "q1": _percentile(values, 0.25),
        "q3": _percentile(values, 0.75),
        "max": max(values),
        "bootstrap_median_ci95": _bootstrap_median_ci(values, seed),
    }


def estimate_capacity_envelope(runs_by_system: dict[str, list[dict]]) -> dict:
    if set(runs_by_system) != set(RATE_SWEEP_SYSTEMS):
        raise ValueError("capacity envelope requires all four frozen systems")
    by_system_rate: dict[str, dict[int, list[dict]]] = {}
    for system, runs in runs_by_system.items():
        by_rate = {rate: [] for rate in RATE_SWEEP_RATES}
        seen_runs: set[tuple[int, int]] = set()
        for run in runs:
            validate_capacity_run_result(run)
            if run["system"] != system:
                raise ValueError("capacity run stored under the wrong system")
            identity = (run["rate_msg_s"], run["run_index"])
            if identity in seen_runs:
                raise ValueError(
                    f"duplicate run_index {run['run_index']} for {system} at {run['rate_msg_s']} msg/s"
                )
            seen_runs.add(identity)
            by_rate[run["rate_msg_s"]].append(run)
        by_system_rate[system] = by_rate

    def raw_classification(rate_runs: list[dict]) -> str:
        if len(rate_runs) != RATE_SWEEP_REPETITIONS:
            return "incomplete"
        intended = sum(run["messages"]["intended"] for run in rate_runs)
        undelivered = sum(run["messages"]["total_undelivered"] for run in rate_runs)
        pooled_loss = undelivered / intended if intended else 1.0
        mean_achieved_ratio = statistics.mean(
            run["rates_msg_s"]["achieved_ratio"] for run in rate_runs
        )
        return "good" if pooled_loss <= 0.01 and mean_achieved_ratio >= 0.99 else "bad"

    mqtt_classifications = {
        rate: raw_classification(by_system_rate["mqtt-loopback"][rate])
        for rate in RATE_SWEEP_RATES
    }
    first_support_bad = next(
        (rate for rate in RATE_SWEEP_RATES if mqtt_classifications[rate] == "bad"), None
    )
    systems = {}
    for system in RATE_SWEEP_SYSTEMS:
        baseline_runs = by_system_rate[system][RATE_SWEEP_BASELINE]
        baseline_p99 = (
            statistics.median(run["latency_ns"]["p99"] for run in baseline_runs)
            if len(baseline_runs) == RATE_SWEEP_REPETITIONS
            else None
        )
        rates = []
        for rate in RATE_SWEEP_RATES:
            rate_runs = by_system_rate[system][rate]
            classification = raw_classification(rate_runs)
            support_confounded = (
                system != "mqtt-loopback"
                and first_support_bad is not None
                and rate >= first_support_bad
            )
            if support_confounded:
                classification = "support-confounded"
            intended = sum(run["messages"]["intended"] for run in rate_runs)
            undelivered = sum(run["messages"]["total_undelivered"] for run in rate_runs)
            pooled_loss = undelivered / intended if intended else None
            mean_achieved_ratio = (
                statistics.mean(run["rates_msg_s"]["achieved_ratio"] for run in rate_runs)
                if rate_runs
                else None
            )
            metrics = {
                "loss": [run["messages"]["total_undelivered"] / run["messages"]["intended"] for run in rate_runs],
                "achieved_rate_msg_s": [run["rates_msg_s"]["achieved"] for run in rate_runs],
                "achieved_ratio": [run["rates_msg_s"]["achieved_ratio"] for run in rate_runs],
                "p99_ns": [run["latency_ns"]["p99"] for run in rate_runs],
            }
            normalized = (
                [value / baseline_p99 for value in metrics["p99_ns"]]
                if baseline_p99
                else []
            )
            rates.append(
                {
                    "rate_msg_s": rate,
                    "run_count": len(rate_runs),
                    "classification": classification,
                    "pooled_loss": pooled_loss,
                    "mean_achieved_ratio": mean_achieved_ratio,
                    "run_summary": {
                        name: _run_summary(values, f"{system}:{rate}:{name}")
                        for name, values in metrics.items()
                    } if rate_runs else None,
                    "normalized_p99": (
                        _run_summary(normalized, f"{system}:{rate}:normalized-p99")
                        if normalized
                        else None
                    ),
                }
            )

        classifications = [entry["classification"] for entry in rates]
        seen_bad = False
        non_monotonic = False
        for classification in classifications:
            if classification == "bad":
                seen_bad = True
            elif classification == "good" and seen_bad:
                non_monotonic = True
        good_rates = [entry["rate_msg_s"] for entry in rates if entry["classification"] == "good"]
        highest_good = max(good_rates, default=None)
        if highest_good is None:
            ceiling_censoring = f"left-censored-below-{RATE_SWEEP_RATES[0]}"
        elif non_monotonic:
            ceiling_censoring = "non-monotonic"
        elif system != "mqtt-loopback" and first_support_bad is not None:
            ceiling_censoring = f"right-censored-above-{highest_good}-by-support-path"
        elif rates[-1]["classification"] == "good":
            ceiling_censoring = f"right-censored-above-{highest_good}"
        else:
            ceiling_censoring = "none"

        eligible = [
            entry
            for entry in rates
            if entry["classification"] not in {"support-confounded", "incomplete"}
        ]
        knee_rate = next(
            (
                entry["rate_msg_s"]
                for entry in eligible
                if entry["normalized_p99"]["median"] > RATE_SWEEP_P99_MULTIPLIER
            ),
            None,
        )
        if knee_rate is not None:
            knee_censoring = "none"
        elif eligible:
            knee_censoring = f"right-censored-above-{eligible[-1]['rate_msg_s']}"
            if system != "mqtt-loopback" and first_support_bad is not None:
                knee_censoring += "-by-support-path"
        else:
            knee_censoring = f"left-censored-below-{RATE_SWEEP_RATES[0]}"

        systems[system] = {
            "complete": all(entry["run_count"] == RATE_SWEEP_REPETITIONS for entry in rates),
            "non_monotonic": non_monotonic,
            "support_censoring": {
                "from_rate_msg_s": first_support_bad,
                "highest_support_uncensored_rate_msg_s": (
                    max((rate for rate in RATE_SWEEP_RATES if first_support_bad is None or rate < first_support_bad), default=None)
                    if system != "mqtt-loopback"
                    else max(RATE_SWEEP_RATES)
                ),
            },
            "delivery_ceiling": {"rate_msg_s": highest_good, "censoring": ceiling_censoring},
            "normalized_p99_knee": {"rate_msg_s": knee_rate, "censoring": knee_censoring},
            "rates": rates,
        }
    return {
        "schema_version": 1,
        "experiment": "e-perf-10",
        "thesis_evidence": True,
        "sample_unit": "run",
        "required_runs_per_rate": RATE_SWEEP_REPETITIONS,
        "rate_points_msg_s": list(RATE_SWEEP_RATES),
        "systems": systems,
    }


def classify_capacity_scout_probe(results: list[dict]) -> str:
    if len(results) != CAPACITY_SCOUT_REPETITIONS:
        raise ValueError("capacity-scout probe requires exactly three complete runs")
    for result in results:
        validate_capacity_scout_result(result)
    return (
        "good"
        if all(
            result["loss_percent"] <= 1.0
            and result["rates_msg_s"]["achieved"] >= 0.99 * result["rates_msg_s"]["intended"]
            for result in results
        )
        else "bad"
    )


def capacity_scout_next_rate(history: list[tuple[int, str]], mqtt: bool = False) -> dict:
    classified = [(int(rate), result) for rate, result in history if result != "invalid"]
    if not classified:
        return {"phase": "geometric", "rate_msg_s": CAPACITY_SCOUT_BASE_RATE}
    last_good = None
    first_bad = None
    confirming_bad = None
    bracket_index = None
    bad_streak = 0
    for index, (rate, result) in enumerate(classified):
        if result == "good":
            last_good = rate
            first_bad = None
            bad_streak = 0
        elif result == "bad":
            if bad_streak == 0:
                first_bad = rate
            bad_streak += 1
            if bad_streak == 2:
                confirming_bad = rate
                bracket_index = index
                break
        else:
            raise ValueError(f"unknown capacity-scout classification: {result}")
    if confirming_bad is None:
        return {"phase": "geometric", "rate_msg_s": classified[-1][0] * 2}
    if last_good is None or first_bad is None:
        return {"phase": "left-censored", "rate_msg_s": CAPACITY_SCOUT_BASE_RATE}
    lower = last_good
    upper = first_bad
    for rate, result in classified[(bracket_index or 0) + 1 :]:
        if not lower < rate < upper:
            continue
        if result == "good":
            lower = rate
        elif result == "bad":
            upper = rate
    target = max(500, (lower + 9) // 10)
    if upper - lower <= target:
        answer = {
            "phase": "resolved",
            "lower_good_rate_msg_s": lower,
            "upper_bad_rate_msg_s": upper,
        }
    else:
        answer = {"phase": "refine", "rate_msg_s": (lower + upper) // 2}
    if mqtt:
        answer["support_censor_above_rate_msg_s"] = last_good
    return answer


def _capacity_scout_config(system: str) -> str:
    return CAPACITY_SCOUT_WAFER_CONFIG if system == "wafer" else _rate_sweep_config(system)


def build_capacity_scout_rate_block(
    rate_msg_s: int, systems: tuple[str, ...] = CAPACITY_SCOUT_SYSTEMS
) -> list[RunItem]:
    if rate_msg_s <= 0:
        raise ValueError("capacity-scout rate must be positive")
    if not systems or any(system not in CAPACITY_SCOUT_SYSTEMS for system in systems):
        raise ValueError("capacity-scout systems are empty or invalid")
    base = sorted(
        systems,
        key=lambda system: (
            hashlib.sha256(f"{CAPACITY_SCOUT_SEED}:{rate_msg_s}:{system}".encode()).hexdigest(),
            system,
        ),
    )
    items = []
    for run_index in range(1, CAPACITY_SCOUT_REPETITIONS + 1):
        ordered = base[run_index - 1 :] + base[: run_index - 1]
        for system in ordered:
            items.append(
                RunItem(
                    experiment="capacity-scout",
                    condition=f"{system}/rate-{rate_msg_s:05d}",
                    run_index=run_index,
                    config=_capacity_scout_config(system),
                    warmup_secs=CAPACITY_SCOUT_WARMUP_SECS,
                    measurement_secs=CAPACITY_SCOUT_MEASUREMENT_SECS,
                    loadgen_profile=RATE_SWEEP_PROFILE,
                    total_messages=rate_msg_s * CAPACITY_SCOUT_MEASUREMENT_SECS,
                    system=system,
                    offered_rate_msg_s=rate_msg_s,
                    exclusive_sut=True,
                )
            )
    return items


def build_capacity_invocation(root: Path, item: RunItem, output: Path) -> dict:
    input_topic = "wafer/telemetry"
    subscriber_topic = input_topic if item.system == "mqtt-loopback" else "wafer/telemetry/hot"
    profile = tomllib.loads((root / str(item.loadgen_profile)).read_text())["loadgen"]
    return {
        "system": item.system,
        "config": item.config,
        "controlled_factors": {
            "broker": "127.0.0.1:1883",
            "topic": input_topic,
            "payload_template_sha256": profile["payload_template_sha256"],
            "qos": 1,
            "warmup_secs": item.warmup_secs,
            "measurement_secs": item.measurement_secs,
            "load_shape": "steady",
            "sequence_example_limit": 1_024,
            "support_cpus": item.support_cpus,
            "sut_cpus": item.runtime_cpus,
        },
        "subscriber_topic": subscriber_topic,
        "publisher_command": loadgen_command(
            root, item, "publish", topic=input_topic,
            summary_file=output / "publisher-summary.json",
        ),
        "subscriber_command": loadgen_command(
            root, item, "subscribe", output=output, topic=subscriber_topic
        ),
    }


def build_swap3_invocation(root: Path, item: RunItem, output: Path) -> dict:
    if item.experiment != "e-swap-3" or item.condition not in SWAP3_STRATEGIES:
        raise ValueError("swap3 invocation requires a frozen E-Swap-3 strategy")
    return {
        "strategy": item.condition,
        "controlled_factors": {
            "broker": "127.0.0.1:1883",
            "input_topic": "wafer/telemetry",
            "output_topic": "wafer/telemetry/hot",
            "rate_msg_s": 1_000,
            "warmup_secs": 30,
            "measurement_secs": 120,
            "event_offset_ns": SWAP3_EVENT_OFFSET_NS,
            "alignment_tolerance_ns": SWAP3_ALIGNMENT_TOLERANCE_NS,
            "bucket_width_ns": SWAP3_BUCKET_WIDTH_NS,
            "coverage_start_offset_ns": SWAP3_COVERAGE_START_NS,
            "coverage_end_offset_ns": SWAP3_COVERAGE_END_NS,
            "publisher_timing_receipt": "publisher-timing.json",
            "action_timing_receipt": "swap_timeline.json",
            "publisher_summary": "publisher-summary.json",
            "subscriber_artifact": "throughput-buckets.json",
        },
        "publisher_command": loadgen_command(root, item, "publish", output=output),
        "subscriber_command": loadgen_command(root, item, "subscribe", output=output),
    }


def build_capacity_scout_invocation(
    root: Path, system: str, rate_msg_s: int, run_index: int, output: Path
) -> dict:
    item = next(
        item
        for item in build_capacity_scout_rate_block(rate_msg_s, (system,))
        if item.run_index == run_index
    )
    return build_capacity_invocation(root, item, output)


def persist_capacity_scout_decision(path: Path, decision: dict) -> bool:
    path.parent.mkdir(parents=True, exist_ok=True)
    encoded = json.dumps(decision, sort_keys=True, separators=(",", ":")) + "\n"
    try:
        with path.open("x") as stream:
            stream.write(encoded)
        return True
    except FileExistsError:
        if path.read_text() != encoded:
            raise ValueError("capacity-scout decision path already contains another decision")
        return False


def _capacity_scout_decision(index: int, kind: str, rate: int, systems: tuple[str, ...], state: dict) -> dict:
    return {
        "schema_version": 1,
        "decision_index": index,
        "batch_class": "capacity-scout",
        "thesis_evidence": False,
        "previous_decision_sha256": None,
        "kind": kind,
        "rate_msg_s": rate,
        "systems": list(systems),
        "state_before": state,
        "schedule": [item.__dict__ for item in build_capacity_scout_rate_block(rate, systems)],
    }


def replay_capacity_scout_decisions(decisions: list[dict], accepted: dict[str, dict]) -> dict:
    histories: dict[str, list[list[int | str]]] = {system: [] for system in CAPACITY_SCOUT_SYSTEMS}
    classifications = []
    pending_mqtt_bad_rate = None
    support_censor_bound = None
    for decision in decisions:
        schedule = decision["schedule"]
        missing = [
            f"{item['experiment']}/{item['condition']}/run-{item['run_index']:02d}"
            for item in schedule
            if f"{item['experiment']}/{item['condition']}/run-{item['run_index']:02d}" not in accepted
        ]
        if missing:
            return {"action": "resume", "decision": decision, "pending_result_keys": missing}
        by_system: dict[str, list[dict]] = {}
        for item in schedule:
            key = f"{item['experiment']}/{item['condition']}/run-{item['run_index']:02d}"
            result = accepted[key]
            validate_capacity_scout_result(result)
            if result["system"] != item["system"] or result["rate_msg_s"] != item["offered_rate_msg_s"]:
                raise ValueError("accepted result does not match persisted decision")
            by_system.setdefault(item["system"], []).append(result)
        classified = {}
        if "mqtt-loopback" in by_system:
            classified["mqtt-loopback"] = classify_capacity_scout_probe(
                by_system["mqtt-loopback"]
            )
        rate = int(decision["rate_msg_s"])
        mqtt_result = classified.get("mqtt-loopback")
        if decision["kind"] == "geometric":
            if mqtt_result is None:
                raise ValueError("geometric decision must include MQTT loopback")
            if mqtt_result == "good":
                classified.update(
                    {
                        system: classify_capacity_scout_probe(results)
                        for system, results in by_system.items()
                        if system != "mqtt-loopback"
                    }
                )
            else:
                classified.update(
                    {
                        system: "support-confounded"
                        for system in by_system
                        if system != "mqtt-loopback"
                    }
                )
            histories["mqtt-loopback"].append([rate, mqtt_result])
            if mqtt_result == "good":
                pending_mqtt_bad_rate = None
                for system in CAPACITY_SCOUT_SUTS:
                    if system in classified:
                        histories[system].append([rate, classified[system]])
            else:
                pending_mqtt_bad_rate = rate
        elif decision["kind"] == "mqtt-confirmation":
            if set(classified) != {"mqtt-loopback"}:
                raise ValueError("MQTT confirmation must be MQTT-only")
            histories["mqtt-loopback"].append([rate, mqtt_result])
            if mqtt_result == "bad" and pending_mqtt_bad_rate is not None:
                good_rates = [
                    candidate_rate
                    for candidate_rate, result in histories["mqtt-loopback"]
                    if result == "good"
                ]
                support_censor_bound = max(good_rates) if good_rates else None
            elif mqtt_result == "good":
                pending_mqtt_bad_rate = None
        elif decision["kind"] == "refinement":
            if len(by_system) != 1:
                raise ValueError("refinement decision must contain one system")
            system, results = next(iter(by_system.items()))
            result = classify_capacity_scout_probe(results)
            classified[system] = result
            histories[system].append([rate, result])
        else:
            raise ValueError(f"unknown capacity-scout decision kind: {decision['kind']}")
        classifications.append({"decision_index": decision["decision_index"], "results": classified})

    referenced = {
        f"{item['experiment']}/{item['condition']}/run-{item['run_index']:02d}"
        for decision in decisions
        for item in decision["schedule"]
    }
    orphaned = sorted(set(accepted) - referenced)
    if orphaned:
        raise ValueError(f"accepted capacity-scout results lack a decision: {orphaned}")

    next_index = len(decisions) + 1
    states = {
        system: capacity_scout_next_rate(history, mqtt=system == "mqtt-loopback")
        for system, history in histories.items()
    }
    mqtt_history = histories["mqtt-loopback"]
    mqtt_bad_pair = any(
        previous[1] == "bad" and current[1] == "bad"
        for previous, current in zip(mqtt_history, mqtt_history[1:])
    )
    if mqtt_bad_pair:
        for system in CAPACITY_SCOUT_SUTS:
            state = states[system]
            exceeds_support = (
                state["phase"] == "refine"
                and (
                    support_censor_bound is None
                    or state["rate_msg_s"] > support_censor_bound
                )
            )
            if state["phase"] == "geometric" or exceeds_support:
                states[system] = {
                    "phase": "support-censored",
                    "censor_above_rate_msg_s": support_censor_bound,
                }

    if mqtt_history and mqtt_history[-1][1] == "bad" and not mqtt_bad_pair:
        rate = mqtt_history[-1][0] * 2
        return {
            "action": "launch",
            "decision": _capacity_scout_decision(
                next_index, "mqtt-confirmation", rate, ("mqtt-loopback",),
                {"histories": histories, "states": states, "classifications": classifications},
            ),
        }

    unresolved = tuple(
        system for system in CAPACITY_SCOUT_SUTS if states[system]["phase"] == "geometric"
    )
    if unresolved and not mqtt_bad_pair:
        rate = states["mqtt-loopback"]["rate_msg_s"]
        return {
            "action": "launch",
            "decision": _capacity_scout_decision(
                next_index, "geometric", rate, ("mqtt-loopback", *unresolved),
                {"histories": histories, "states": states, "classifications": classifications},
            ),
        }

    refinement_order = ("mqtt-loopback", *CAPACITY_SCOUT_SUTS)
    for system in refinement_order:
        state = states[system]
        if state["phase"] == "refine":
            return {
                "action": "launch",
                "decision": _capacity_scout_decision(
                    next_index,
                    "refinement",
                    state["rate_msg_s"],
                    (system,),
                    {"histories": histories, "states": states, "classifications": classifications},
                ),
            }

    if all(
        states[system]["phase"] in {"resolved", "left-censored", "support-censored"}
        for system in CAPACITY_SCOUT_SUTS
    ):
        return {
            "action": "stop",
            "reason": "all-suts-resolved-or-support-censored",
            "states": states,
            "classifications": classifications,
        }
    raise ValueError("capacity-scout replay reached an unhandled state")


def validate_capacity_scout_decision_replay(
    decisions: list[dict], accepted: dict[str, dict]
) -> None:
    comparable_fields = {
        "schema_version",
        "decision_index",
        "batch_class",
        "thesis_evidence",
        "kind",
        "rate_msg_s",
        "systems",
        "state_before",
        "schedule",
    }
    prefix: list[dict] = []
    referenced: set[str] = set()
    for decision in decisions:
        prefix_accepted = {key: accepted[key] for key in referenced if key in accepted}
        expected = replay_capacity_scout_decisions(prefix, prefix_accepted)
        if expected["action"] != "launch":
            raise ValueError("capacity-scout decision exists before its predecessor completed")
        expected_decision = expected["decision"]
        if any(decision.get(field) != expected_decision.get(field) for field in comparable_fields):
            raise ValueError("capacity-scout decision was not derived from immutable history")
        prefix.append(decision)
        referenced.update(
            f"{item['experiment']}/{item['condition']}/run-{item['run_index']:02d}"
            for item in decision["schedule"]
        )


def capacity_scout_safety_action(snapshot: dict) -> dict:
    if snapshot["provenance_matches"] is not True:
        return {"action": "stop", "reason": "provenance-drift"}
    if snapshot["telemetry_available"] is not True:
        return {"action": "stop", "reason": "telemetry-unavailable"}
    if snapshot["throttled"] is True:
        return {"action": "stop", "reason": "throttling"}
    if snapshot["temperature_millicelsius"] >= 75_000:
        return {"action": "stop", "reason": "temperature-75c"}
    if snapshot.get("attempt_elapsed_secs", 0) >= CAPACITY_SCOUT_ATTEMPT_TIMEOUT_SECS:
        return {"action": "stop", "reason": "attempt-240s"}
    if snapshot["elapsed_secs"] >= CAPACITY_SCOUT_BATCH_TIMEOUT_SECS:
        return {"action": "stop", "reason": "batch-18h"}
    required_free = max(CAPACITY_SCOUT_DISK_FLOOR_BYTES, 2 * snapshot["largest_probe_bytes"])
    if snapshot["free_bytes"] < required_free:
        return {"action": "stop", "reason": "disk-floor", "required_free_bytes": required_free}
    if snapshot["repeated_systemic_failures"] >= 3:
        return {"action": "stop", "reason": "repeated-systemic-failure"}
    if snapshot["temperature_millicelsius"] >= 70_000:
        return {"action": "pause", "reason": "temperature-cool-below-65c"}
    return {"action": "proceed"}


def write_capacity_scout_progress(
    ledger: Path,
    event: str,
    item: RunItem | None,
    attempt: int | None,
    temperature_millicelsius: int | None,
    throttled: bool | None,
    counters: dict | None = None,
    error: str | None = None,
) -> None:
    entry = {
        "timestamp": utc_now(),
        "event": event,
        "probe": f"{item.system}@{item.offered_rate_msg_s}" if item else None,
        "condition": item.condition if item else None,
        "run": item.run_index if item else None,
        "attempt": attempt,
        "temperature_millicelsius": temperature_millicelsius,
        "throttled": throttled,
        "counters": counters,
        "error": error,
    }
    with (ledger / "progress.jsonl").open("a") as stream:
        stream.write(json.dumps(entry, separators=(",", ":")) + "\n")
    print(
        f"[{entry['timestamp']}] SCOUT event={event} probe={entry['probe']} "
        f"run={entry['run']} attempt={attempt} temp_mc={temperature_millicelsius} "
        f"throttled={throttled} counters={counters} error={error or '-'}",
        flush=True,
    )

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


def analyze_swap3_disruption(
    throughput: dict,
    timeline: dict,
    publisher: dict,
    subscriber: dict,
) -> dict:
    buckets = throughput.get("buckets")
    if not isinstance(buckets, list) or len(buckets) != 200:
        raise ValueError("E-Swap-3 requires exactly 200 throughput buckets")
    rates = [float(bucket["rate_msg_s"]) for bucket in buckets]
    baseline = statistics.median(rates[:80])
    if baseline <= 0:
        raise ValueError("E-Swap-3 baseline rate must be positive")
    event_min = min(rates[80:120])
    threshold = 0.95 * baseline
    below = [rate < threshold for rate in rates]
    interruption_start = 100
    interruption_end = 100
    if below[100]:
        while interruption_start > 0 and below[interruption_start - 1]:
            interruption_start -= 1
        while interruption_end < len(below) and below[interruption_end]:
            interruption_end += 1
    interruption_buckets = interruption_end - interruption_start

    action_end_offset_ns = int(timeline["action_end_offset_ns"])
    first_recovery_offset_ns = None
    for index in range(200 - 4):
        start = int(buckets[index]["start_offset_ns"])
        if start < action_end_offset_ns:
            continue
        if all(float(buckets[candidate]["rate_msg_s"]) >= threshold for candidate in range(index, index + 5)):
            first_recovery_offset_ns = start
            break
    censored = first_recovery_offset_ns is None
    recovery_end_ns = SWAP3_COVERAGE_END_NS if censored else first_recovery_offset_ns

    intended = int(publisher["intended"])
    rejected = int(publisher["rejected"])
    enqueued = int(publisher["enqueued"])
    received_events = int(subscriber["total_recorded"])
    duplicates = int(subscriber["sequence"]["total_duplicates"])
    received_unique = received_events - duplicates
    if intended != rejected + enqueued or received_unique > enqueued:
        raise ValueError("E-Swap-3 sequence totals do not reconcile")
    return {
        "schema_version": 1,
        "strategy": timeline["strategy"],
        "baseline_rate_msg_s": baseline,
        "event_min_rate_msg_s": event_min,
        "dip_percent": 100.0 * max(0.0, baseline - event_min) / baseline,
        "interruption_ns": interruption_buckets * SWAP3_BUCKET_WIDTH_NS,
        "recovery_ns": max(0, recovery_end_ns - action_end_offset_ns),
        "recovery_right_censored": censored,
        "action_duration_ns": int(timeline["action_duration_ns"]),
        "loss": intended - received_unique,
        "duplicates": duplicates,
        "messages": {
            "intended": intended,
            "rejected": rejected,
            "enqueued": enqueued,
            "received_events": received_events,
            "received_unique": received_unique,
        },
        "latency_ns": {
            "p50": int(subscriber["latency_p50_ns"]),
            "p95": int(subscriber["latency_p95_ns"]),
            "p99": int(subscriber["latency_p99_ns"]),
        },
    }


def validate_swap3_artifacts(
    throughput: dict,
    timeline: dict,
    publisher: dict,
    subscriber: dict,
) -> None:
    if (
        throughput.get("clock") != "unix-epoch"
        or throughput.get("clock_purpose") != "cross-process-alignment"
    ):
        raise ValueError("E-Swap-3 buckets must declare unix-epoch alignment clock")
    if throughput.get("bucket_width_ns") != SWAP3_BUCKET_WIDTH_NS:
        raise ValueError("E-Swap-3 bucket width must be 100 ms")
    if (
        throughput.get("scheduled_event_offset_ns") != SWAP3_EVENT_OFFSET_NS
        or throughput.get("alignment_tolerance_ns") != SWAP3_ALIGNMENT_TOLERANCE_NS
        or throughput.get("coverage_start_offset_ns") != SWAP3_COVERAGE_START_NS
        or throughput.get("coverage_end_offset_ns") != SWAP3_COVERAGE_END_NS
    ):
        raise ValueError("E-Swap-3 event placement or coverage differs from the frozen method")
    buckets = throughput.get("buckets")
    if not isinstance(buckets, list) or len(buckets) != 200:
        raise ValueError("E-Swap-3 requires exactly 200 throughput buckets")
    expected_start = SWAP3_COVERAGE_START_NS
    totals = {"received_unique": 0, "received_events": 0, "duplicates": 0}
    for bucket in buckets:
        if bucket.get("start_offset_ns") != expected_start or bucket.get("end_offset_ns") != expected_start + SWAP3_BUCKET_WIDTH_NS:
            raise ValueError("E-Swap-3 buckets must be contiguous 100 ms intervals")
        events = int(bucket["received_events"])
        unique = int(bucket["received_unique"])
        duplicates = int(bucket["duplicates"])
        if events != unique + duplicates or not math.isclose(float(bucket["rate_msg_s"]), unique * 10.0):
            raise ValueError("E-Swap-3 bucket counters or rate do not reconcile")
        for field in totals:
            totals[field] += int(bucket[field])
        expected_start += SWAP3_BUCKET_WIDTH_NS
    if any(int(throughput.get(field, -1)) != total for field, total in totals.items()):
        raise ValueError("E-Swap-3 bucket totals do not reconcile")
    if timeline.get("strategy") not in SWAP3_STRATEGIES:
        raise ValueError("E-Swap-3 timeline strategy is invalid")
    if (
        timeline.get("timestamp_clock") != "unix-epoch"
        or timeline.get("timestamp_clock_purpose") != "cross-process-alignment"
        or timeline.get("scheduling_clock") != "monotonic"
        or timeline.get("duration_clock") != "monotonic"
    ):
        raise ValueError("E-Swap-3 timeline clocks are invalid")
    event_timestamp = int(throughput["event_timestamp_ns"])
    measurement_start = int(timeline.get("measurement_start_timestamp_ns", -1))
    scheduled_timestamp = int(timeline.get("scheduled_event_timestamp_ns", -1))
    actual_offset = event_timestamp - measurement_start
    alignment_error = event_timestamp - scheduled_timestamp
    if (
        int(timeline.get("event_timestamp_ns", -1)) != event_timestamp
        or int(timeline.get("action_start_timestamp_ns", -1)) != event_timestamp
        or int(timeline.get("scheduled_event_offset_ns", -1)) != SWAP3_EVENT_OFFSET_NS
        or scheduled_timestamp != measurement_start + SWAP3_EVENT_OFFSET_NS
        or int(timeline.get("event_offset_from_measurement_start_ns", -1)) != actual_offset
        or int(timeline.get("alignment_error_ns", SWAP3_ALIGNMENT_TOLERANCE_NS + 1))
            != alignment_error
        or int(timeline.get("alignment_tolerance_ns", -1)) != SWAP3_ALIGNMENT_TOLERANCE_NS
        or abs(alignment_error) > SWAP3_ALIGNMENT_TOLERANCE_NS
    ):
        raise ValueError("E-Swap-3 event clocks do not align")
    matching_fields = (
        "measurement_start_timestamp_ns",
        "scheduled_event_timestamp_ns",
        "scheduled_event_offset_ns",
        "event_offset_from_measurement_start_ns",
        "alignment_error_ns",
        "alignment_tolerance_ns",
    )
    if any(throughput.get(field) != timeline.get(field) for field in matching_fields):
        raise ValueError("E-Swap-3 bucket and action alignment metadata differ")
    action_duration = int(timeline.get("action_duration_ns", -1))
    action_end_offset = int(timeline.get("action_end_offset_ns", -1))
    action_end_timestamp = int(timeline.get("action_end_timestamp_ns", -1))
    action_start_monotonic = int(timeline.get("action_start_monotonic_ns", -1))
    action_end_monotonic = int(timeline.get("action_end_monotonic_ns", -1))
    if (
        action_duration < 0
        or action_end_offset < 0
        or action_end_timestamp < event_timestamp
        or action_end_offset != action_end_timestamp - event_timestamp
        or action_end_monotonic < action_start_monotonic
        or action_duration != action_end_monotonic - action_start_monotonic
    ):
        raise ValueError("E-Swap-3 action timing does not reconcile")
    analysis = analyze_swap3_disruption(throughput, timeline, publisher, subscriber)
    if int(subscriber["sequence"]["total_received"]) != int(subscriber["total_recorded"]):
        raise ValueError("E-Swap-3 subscriber sequence totals do not reconcile")
    if int(throughput["received_events"]) > int(subscriber["total_recorded"]):
        raise ValueError("E-Swap-3 bucket population exceeds the measured sequence population")
    if analysis["loss"] < 0:
        raise ValueError("E-Swap-3 received population exceeds publisher population")


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


def _validate_rate_sweep_definition(_root: Path, definition: dict) -> None:
    expected = {
        "systems": list(RATE_SWEEP_SYSTEMS),
        "rate_points_msg_s": list(RATE_SWEEP_RATES),
        "repetitions": 30,
        "sample_unit": "run",
        "thesis_evidence": True,
    }
    for field, value in expected.items():
        if definition.get(field) != value:
            raise ValueError(f"e-perf-10 {field} differs from the final canonical matrix")
    criteria = definition.get("capacity_envelope", {})
    for field, value in {
        "baseline_rate_msg_s": 1_000,
        "max_loss_percent": 1.0,
        "min_achieved_ratio": 0.99,
        "normalized_p99_knee_multiplier": 2.0,
        "support_path_censoring": "mqtt-loopback",
        "competitive_ratio_threshold": 0.70,
    }.items():
        if criteria.get(field) != value:
            raise ValueError(f"e-perf-10 capacity_envelope {field} differs from the final matrix")


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
            "eval/configs/pipeline-c-passthrough.toml"
            if mode == "both"
            else f"eval/configs/pipeline-c-{mode}.toml",
        )
        for mode in ("neither", "fuel-only", "epoch-only", "both")
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
            "eval/loadgen/telemetry-120b.toml",
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
        Condition("burst-2x", "eval/configs/e-swap/pipeline-hotswap-burst.toml"),
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
    "e-density-1": (Condition("release-components", ""),),
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
    "e-density-1",
)


def build_schedule(experiments: set[str], seed: int) -> list[RunItem]:
    unknown = experiments - CONDITIONS.keys()
    if unknown:
        raise ValueError(f"unsupported experiments: {', '.join(sorted(unknown))}")

    matrix_path = Path(__file__).resolve().parents[2] / "canonical-matrix.json"
    matrix_document = json.loads(matrix_path.read_text())
    matrix = matrix_document["experiments"]
    if "e-perf-10" in experiments:
        _validate_rate_sweep_definition(matrix_path.parents[1], matrix["e-perf-10"])
    wafer_configs = {
        (entry["experiment"], entry["condition"]): entry["config"]
        for entry in matrix_document["final_campaign"]["wafer_config_catalog"]
    }
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
                        config=(
                            wafer_configs[(experiment, condition.name)]
                            if condition.system == "wafer" and condition.config
                            else condition.config
                        ),
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
    focused = json.loads(CANONICAL_MATRIX_PATH.read_text())["focused_pilot"]
    if seed != focused["seed"]:
        raise ValueError(f"focused pilot seed must be {focused['seed']}")
    snapshot = CANONICAL_MATRIX_PATH.with_name("focused-pilot-schedule.json")
    return [RunItem(**item) for item in json.loads(snapshot.read_text())]


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
    source_node: str,
    target_messages: int,
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
    if total_messages != sequence["total_received"]:
        raise ValueError(
            f"{branch_dir} throughput and sequence populations differ: "
            f"{total_messages} != {sequence['total_received']}"
        )
    if latency["sample_count"] != sequence["total_received"]:
        raise ValueError(
            f"{branch_dir} latency and sequence populations differ: "
            f"{latency['sample_count']} != {sequence['total_received']}"
        )
    duration_seconds = (
        (window["finished_ns"] - window["started_ns"]) / 1_000_000_000
        if window
        else 0.0
    )
    offered = sequence["total_expected"]
    return {
        "artifact_dir": branch_dir,
        "source_node": source_node,
        "target_messages": target_messages,
        "target_shortfall_messages": max(0, target_messages - offered),
        "sequence_scope": "post_warmup",
        "offered_messages": offered,
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
    branch_sources: dict[str, str],
    target_messages: int,
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
            "branch_a": _summarize_branch_artifacts(
                output, "branch-a", branch_sources["branch-a"], target_messages
            ),
            "branch_b": _summarize_branch_artifacts(
                output, "branch-b", branch_sources["branch-b"], target_messages
            ),
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
    if result["offered_rate_msg_s"] not in HISTORICAL_RATE_SWEEP_RATES:
        raise ValueError("historical rate-sweep result offered_rate_msg_s is not frozen")

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


def wait_for_ekuiper_rule_ready(rule_id: str, timeout_secs: float = 10.0) -> None:
    url = f"http://127.0.0.1:9081/rules/{rule_id}/status"
    deadline = time.monotonic() + timeout_secs
    while time.monotonic() < deadline:
        try:
            status = _url_value(url)
            if isinstance(status, dict) and str(status.get("status", "")).lower() == "running":
                return
        except (OSError, ValueError, urllib.error.URLError):
            pass
        time.sleep(0.1)
    raise RuntimeError(f"eKuiper rule did not become ready: {rule_id}")


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


def write_json_atomic(path: Path, value: dict) -> None:
    temporary = path.with_suffix(f"{path.suffix}.tmp")
    temporary.write_text(json.dumps(value, indent=2) + "\n")
    os.replace(temporary, path)


def loadgen_command(
    root: Path,
    item: RunItem,
    action: str,
    output: Path | None = None,
    duration: int | None = None,
    topic: str | None = None,
    trace_file: Path | None = None,
    sequence_start: int | None = None,
    summary_file: Path | None = None,
    event_aligned: bool = True,
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
        if item.experiment in {"e-perf-10", "capacity-scout"} and item.total_messages is not None:
            command.extend(["--sequence-end-exclusive", str(item.total_messages)])
        if item.experiment in {"e-perf-10", "capacity-scout", "e-swap-3"}:
            command.extend(["--sequence-example-limit", "1024"])
        if item.experiment == "e-swap-3" and event_aligned:
            command.extend([
                "--publisher-timing-receipt", str(output / "publisher-timing.json"),
                "--action-timing-receipt", str(output / "swap_timeline.json"),
            ])
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
    if item.experiment in {"e-perf-10", "capacity-scout"}:
        command.append("--drop-when-full")
    if item.experiment == "e-swap-3" and event_aligned:
        timing_dir = output or (summary_file.parent if summary_file is not None else None)
        if timing_dir is None:
            raise ValueError("E-Swap-3 publisher requires an output directory")
        command.extend([
            "--timing-receipt", str(timing_dir / "publisher-timing.json"),
            "--summary-file", str(timing_dir / "publisher-summary.json"),
        ])
    if sequence_start is not None:
        command.extend(["--sequence-start", str(sequence_start)])
    if trace_file is not None:
        command.extend(["--trace-file", str(trace_file)])
    if summary_file is not None:
        command.extend(["--summary-file", str(summary_file)])
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
    publisher: subprocess.Popen | None = None
    subscriber: subprocess.Popen | None = None
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
                loadgen_command(
                    root, item, "publish", duration=item.warmup_secs, event_aligned=False
                ),
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
                loadgen_command(root, item, "publish", output=output),
                cwd=root,
                env=environment,
                stdout=log,
                stderr=log,
            )
            timing_path = output / "publisher-timing.json"
            deadline = time.monotonic() + 10
            while not timing_path.is_file() and time.monotonic() < deadline:
                time.sleep(0.01)
            timing = json.loads(timing_path.read_text())
            if timing.get("event_offset_ns") != SWAP3_EVENT_OFFSET_NS:
                raise RuntimeError("publisher timing receipt does not declare measured t=60")
            scheduled_event_ns = int(timing["event_unix_epoch_ns"])
            wait_secs = max(0.0, (scheduled_event_ns - time.time_ns()) / 1_000_000_000)
            action_wait_target = time.monotonic() + wait_secs
            time.sleep(max(0.0, action_wait_target - time.monotonic()))
            action_started_ns = time.time_ns()
            action_started_monotonic_ns = time.monotonic_ns()
            alignment_error_ns = action_started_ns - scheduled_event_ns
            if item.condition == "wafer-hotswap":
                plugin = root / "plugins/pass-through-v2/target/wasm32-wasip2/release/wafer_pass_through_v2.wasm"
                response = post_hot_swap("transform", plugin)
                if response["http_status"] != 200:
                    raise RuntimeError(f"hot-swap failed: {response}")
            elif is_ekuiper:
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
                wait_for_ekuiper_rule_ready("pipeline_a")
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
                wait_for_api("http://127.0.0.1:9090/health")
                if runtime.poll() is not None:
                    raise RuntimeError("wafer runtime failed to restart")
            action_finished_monotonic_ns = time.monotonic_ns()
            action_finished_ns = time.time_ns()
            action_duration_ns = action_finished_monotonic_ns - action_started_monotonic_ns
            if action_finished_ns < action_started_ns:
                raise RuntimeError("wall clock moved backwards during E-Swap-3 action")
            action_end_offset_ns = action_finished_ns - action_started_ns
            event_offset_ns = action_started_ns - int(timing["measurement_started_unix_epoch_ns"])
            action_timing = {
                "schema_version": 1,
                "strategy": item.condition,
                "timestamp_clock": "unix-epoch",
                "timestamp_clock_purpose": "cross-process-alignment",
                "scheduling_clock": "monotonic",
                "duration_clock": "monotonic",
                "measurement_start_timestamp_ns": int(timing["measurement_started_unix_epoch_ns"]),
                "scheduled_event_offset_ns": SWAP3_EVENT_OFFSET_NS,
                "scheduled_event_timestamp_ns": scheduled_event_ns,
                "event_timestamp_ns": action_started_ns,
                "event_offset_from_measurement_start_ns": event_offset_ns,
                "alignment_error_ns": alignment_error_ns,
                "alignment_tolerance_ns": SWAP3_ALIGNMENT_TOLERANCE_NS,
                "action_start_timestamp_ns": action_started_ns,
                "action_end_timestamp_ns": action_finished_ns,
                "action_end_offset_ns": action_end_offset_ns,
                "action_start_monotonic_ns": action_started_monotonic_ns,
                "action_end_monotonic_ns": action_finished_monotonic_ns,
                "action_duration_ns": action_duration_ns,
            }
            write_json_atomic(output / "swap_timeline.json", action_timing)
            if abs(alignment_error_ns) > SWAP3_ALIGNMENT_TOLERANCE_NS:
                raise RuntimeError(
                    "actual E-Swap-3 action start missed measured t=60 by more than 10 ms"
                )
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
        for process in (publisher, subscriber, runtime):
            if process is not None and process.poll() is None:
                process.terminate()
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


def _read_pi_thermal(path: Path) -> dict:
    with path.open(newline="") as stream:
        rows = list(csv.DictReader(stream))
    if not rows:
        raise ValueError("pi-telemetry.csv contains no samples")
    return {
        "max_temperature_millicelsius": max(
            int(row["temperature_millicelsius"]) for row in rows
        ),
        "throttled": any(row.get("throttled") != "0x0" for row in rows),
    }


def _rate_sweep_throttled(path: Path) -> bool:
    return bool(_read_pi_thermal(path)["throttled"])


def _binary_file_receipt(path: Path, samples: int | None = None) -> dict:
    receipt = {
        "path": path.name,
        "sha256": hashlib.sha256(path.read_bytes()).hexdigest(),
    }
    if samples is not None:
        receipt["samples"] = samples
    return receipt


def _write_capacity_result(
    item: RunItem,
    output: Path,
    controlled_factors: dict,
    *,
    final: bool,
) -> dict:
    if (output / "published.csv").exists() or (output / "received.csv").exists():
        raise ValueError("capacity run must not contain per-message traces")
    publisher = json.loads((output / "publisher-summary.json").read_text())
    subscriber = json.loads((output / "subscriber-metadata.json").read_text())
    measurement_duration_ns = int(publisher["measurement_duration_ns"])
    summary = analyze_capacity_scout_summary(publisher, subscriber, measurement_duration_ns)
    config = output / "config.toml"
    profile = output / "loadgen-profile.toml"
    process_audit = output / "process-audit.json"
    metadata = json.loads((output / "metadata.json").read_text())
    resources = summarize_process_resources(output / "resource-usage.csv")
    expected_scope = "no-sut" if item.system == "mqtt-loopback" else "sut"
    if resources["scope"] != expected_scope:
        raise ValueError(
            f"capacity resource scope {resources['scope']!r}, expected {expected_scope!r}"
        )
    result = {
        "schema_version": 1,
        "batch_class": "final-capacity" if final else "capacity-scout",
        "thesis_evidence": final,
        "system": item.system,
        "rate_msg_s": item.offered_rate_msg_s,
        "run_index": item.run_index,
        "source_git_sha": metadata.get("git_sha"),
        "source_dirty": metadata.get("git_dirty"),
        "measurement_duration_ns": measurement_duration_ns,
        **summary,
        "latency_hdr": {
            **_binary_file_receipt(
                output / "latency.hdr", summary["messages"]["received_events"]
            ),
            "lowest_ns": int(subscriber["histogram_lowest_ns"]),
            "highest_ns": int(subscriber["histogram_highest_ns"]),
            "significant_digits": int(subscriber["histogram_sig_digits"]),
        },
        "resources": resources,
        "thermal": _read_pi_thermal(output / "pi-telemetry.csv"),
        "process_audit": _binary_file_receipt(process_audit),
        "config": _binary_file_receipt(config),
        "loadgen_profile": _binary_file_receipt(profile),
        "provenance": _binary_file_receipt(output / "metadata.json"),
        "controlled_factors": controlled_factors,
        "traces": False,
    }
    if final:
        result["experiment"] = "e-perf-10"
        result["rates_msg_s"]["achieved_ratio"] = (
            result["rates_msg_s"]["achieved"] / item.offered_rate_msg_s
        )
        validate_capacity_run_result(result)
        name = "capacity-run.json"
    else:
        validate_capacity_scout_result(result)
        name = "capacity-scout.json"
    (output / name).write_text(json.dumps(result, indent=2) + "\n")
    if final:
        verify_capacity_result_files(output, result)
    return result


def write_capacity_scout_result(
    root: Path,
    item: RunItem,
    output: Path,
    measurement_duration_ns: int,
    controlled_factors: dict,
) -> dict:
    del root
    publisher = json.loads((output / "publisher-summary.json").read_text())
    publisher.setdefault("measurement_duration_ns", measurement_duration_ns)
    (output / "publisher-summary.json").write_text(json.dumps(publisher, indent=2) + "\n")
    return _write_capacity_result(item, output, controlled_factors, final=False)


def write_capacity_result(item: RunItem, output: Path, controlled_factors: dict) -> dict:
    return _write_capacity_result(item, output, controlled_factors, final=True)


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
    final_capacity = item.experiment == "e-perf-10" and FOCUSED_MATRIX_SHA_ENV not in os.environ
    if item.experiment == "capacity-scout" or final_capacity:
        shutil.copy2(root / str(item.loadgen_profile), output / "loadgen-profile.toml")
        invocation = build_capacity_invocation(root, item, output)
        (output / "invocation-receipt.json").write_text(
            json.dumps(invocation, indent=2) + "\n"
        )
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
                    trace_file=(output / "received.csv") if item.experiment == "e-perf-10" and not final_capacity else None,
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
                    trace_file=(output / "published.csv") if item.experiment == "e-perf-10" and not final_capacity else None,
                    summary_file=(output / "publisher-summary.json") if item.experiment == "capacity-scout" or final_capacity else None,
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
        if item.experiment == "capacity-scout":
            invocation = json.loads((output / "invocation-receipt.json").read_text())
            result = write_capacity_scout_result(
                root, item, output, measurement_duration_ns, invocation["controlled_factors"]
            )
            if result["thermal"]["throttled"]:
                raise RuntimeError("Pi throttling occurred during capacity-scout measurement")
        elif final_capacity:
            invocation = json.loads((output / "invocation-receipt.json").read_text())
            write_capacity_result(item, output, invocation["controlled_factors"])
            verify_result(root, output)
        else:
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
        TimeoutError,
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

    if item.experiment in {"e-perf-10", "capacity-scout"}:
        return run_rate_sweep_item(root, item, selection)
    if item.experiment in {"e-swap-1", "e-swap-4", "e-swap-5"}:
        return run_hot_swap_item(root, item, selection)
    if item.experiment == "e-swap-3":
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
    if item.experiment in {"e-perf-10", "capacity-scout"}:
        metadata["thesis_evidence"] = item.experiment == "e-perf-10"
        metadata["offered_rate_msg_s"] = item.offered_rate_msg_s
        metadata["batch_class"] = (
            "final-capacity" if item.experiment == "e-perf-10" else "capacity-scout"
        )
    if item.experiment == "e-swap-3":
        metadata["disruption_capture"] = build_swap3_invocation(root, item, output)[
            "controlled_factors"
        ]
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
            "source_nodes": {"branch_a": "source_a", "branch_b": "source_b"},
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
        branch_sources = {
            branch_dir: next(
                edge["from"]
                for edge in config["edges"]
                if edge["to"] == branch_node
                and config["nodes"][edge["from"]].get("kind") == "bench-source"
            )
            for branch_dir, branch_node in (
                ("branch-a", "branch_a"),
                ("branch-b", "branch_b"),
            )
        }
        source_configs = [config["nodes"][source] for source in branch_sources.values()]
        target_messages = {
            int(float(source["rate"]) * item.measurement_secs)
            for source in source_configs
        }
        if len(target_messages) != 1:
            raise ValueError("E-Iso-7 branch sources must use the same target measurement load")
        branch_isolation = derive_branch_isolation(
            output,
            warmup_secs=item.warmup_secs,
            measurement_secs=item.measurement_secs,
            branch_sources=branch_sources,
            target_messages=target_messages.pop(),
        )
        branch_isolation["condition"] = item.condition
        branch_isolation["run_index"] = item.run_index
        (output / "branch-isolation.json").write_text(
            json.dumps(branch_isolation, indent=2) + "\n"
        )
        branch_a_window = output / "branch-a/measurement-window.json"
        if branch_a_window.is_file():
            shutil.copyfile(branch_a_window, output / "measurement-window.json")

    if item.experiment == "e-swap-3":
        timeline = json.loads((output / "swap_timeline.json").read_text())
        throughput = json.loads((output / "throughput-buckets.json").read_text())
        publisher = json.loads((output / "publisher-summary.json").read_text())
        subscriber = json.loads((output / "subscriber-metadata.json").read_text())
        validate_swap3_artifacts(throughput, timeline, publisher, subscriber)
        analysis = analyze_swap3_disruption(throughput, timeline, publisher, subscriber)
        (output / "disruption-timeline.json").write_text(json.dumps(timeline, indent=2) + "\n")
        (output / "disruption-analysis.json").write_text(json.dumps(analysis, indent=2) + "\n")

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
    capacity_paths = list(result_root.rglob("capacity-run.json"))
    if capacity_paths:
        for path in capacity_paths:
            status_path = path.parent / "canonical-status.json"
            try:
                if json.loads(status_path.read_text()).get("status") != "passed":
                    continue
                result = json.loads(path.read_text())
                validate_capacity_run_result(result)
            except (OSError, ValueError, KeyError, TypeError):
                continue
            by_system[result["system"]].append(result)
        summary = {
            **estimate_capacity_envelope(by_system),
            "batch_id": batch_id,
            "criteria": {
                "max_pooled_loss": RATE_SWEEP_MAX_LOSS_PERCENT / 100,
                "min_mean_achieved_ratio": 0.99,
                "normalized_p99_knee_multiplier": RATE_SWEEP_P99_MULTIPLIER,
            },
        }
        path = root / "eval/results/canonical-batches" / f"rpi5-{batch_id}" / "rate-sweep-summary.json"
        path.write_text(json.dumps(summary, indent=2) + "\n")
        return path

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
    matrix_bytes = matrix_path.read_bytes()
    matrix_sha256 = hashlib.sha256(matrix_bytes).hexdigest()
    if receipt.get("status") != "frozen-before-execution":
        raise ValueError("focused-pilot freeze receipt is not frozen-before-execution")
    if receipt.get("canonical_matrix_sha256") != matrix_sha256:
        matrix = json.loads(matrix_bytes)
        selection = json.dumps(
            matrix.get("focused_pilot", {}).get("experiments", {}),
            sort_keys=True,
            separators=(",", ":"),
        ).encode()
        if (
            "final_campaign" not in matrix
            or receipt.get("focused_selection_sha256")
            != hashlib.sha256(selection).hexdigest()
        ):
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


def load_capacity_scout_replay(root: Path, batch_id: str) -> tuple[list[dict], dict[str, dict]]:
    batch_root = root / "eval/results/capacity-scout" / f"rpi5-{batch_id}"
    decisions = []
    previous_sha256 = None
    for path in sorted((batch_root / "decisions").glob("decision-*.json")):
        raw = path.read_bytes()
        decision = json.loads(raw)
        if decision.get("decision_index") != len(decisions) + 1:
            raise ValueError("capacity-scout decisions are missing, duplicated, or out of order")
        if decision.get("previous_decision_sha256") != previous_sha256:
            raise ValueError("capacity-scout decision hash chain is invalid")
        expected_schedule = [
            item.__dict__
            for item in build_capacity_scout_rate_block(
                int(decision["rate_msg_s"]), tuple(decision["systems"])
            )
        ]
        if decision.get("schedule") != expected_schedule:
            raise ValueError("capacity-scout decision schedule is not reproducible")
        decisions.append(decision)
        previous_sha256 = hashlib.sha256(raw).hexdigest()
    accepted_paths = {}
    for path in batch_root.rglob("capacity-scout.json"):
        status_path = path.parent / "canonical-status.json"
        if not status_path.is_file() or json.loads(status_path.read_text()).get("status") != "passed":
            continue
        relative = path.parent.relative_to(batch_root)
        parts = relative.parts
        if len(parts) != 3:
            raise ValueError(f"unexpected capacity-scout result path: {relative}")
        system, rate_part, run_part = parts
        logical = f"capacity-scout/{system}/{rate_part}/{run_part.rsplit('-attempt-', 1)[0]}"
        if logical in accepted_paths:
            raise ValueError(f"duplicate accepted capacity-scout logical run: {logical}")
        accepted_paths[logical] = path
    accepted = {}
    batch_path = batch_root / "batch.json"
    expected_sha = json.loads(batch_path.read_text())["source_git_sha"] if batch_path.is_file() else None
    for logical, path in accepted_paths.items():
        result = json.loads(path.read_text())
        verify_capacity_scout_result_files(path.parent, result)
        if expected_sha is not None and result["source_git_sha"] != expected_sha:
            raise ValueError("capacity-scout result source SHA differs from batch")
        accepted[logical] = result
    validate_capacity_scout_decision_replay(decisions, accepted)
    return decisions, accepted


def capacity_scout_source_state(root: Path) -> dict:
    try:
        sha = subprocess.check_output(
            ["git", "-C", str(root), "rev-parse", "HEAD"],
            text=True,
            stderr=subprocess.DEVNULL,
        ).strip()
        dirty = bool(
            subprocess.check_output(
                ["git", "-C", str(root), "status", "--porcelain"],
                text=True,
                stderr=subprocess.DEVNULL,
            ).strip()
        )
        tags = [
            tag
            for tag in subprocess.check_output(
                ["git", "-C", str(root), "tag", "--points-at", sha],
                text=True,
                stderr=subprocess.DEVNULL,
            ).splitlines()
            if tag
        ]
        state = {"git_sha": sha, "git_dirty": dirty, "git_tags": tags}
    except (OSError, subprocess.CalledProcessError):
        try:
            state = json.loads((root / "SOURCE_STATE.json").read_text())
        except (OSError, ValueError) as error:
            raise ValueError("capacity-scout source provenance is unavailable") from error
    if not re.fullmatch(r"[0-9a-f]{40}", str(state.get("git_sha", ""))):
        raise ValueError("capacity-scout source SHA is invalid")
    if not isinstance(state.get("git_dirty"), bool):
        raise ValueError("capacity-scout source dirty flag is invalid")
    tags = state.get("git_tags")
    if not isinstance(tags, list) or not all(isinstance(tag, str) and tag for tag in tags):
        raise ValueError("capacity-scout source tags are invalid")
    return {"git_sha": state["git_sha"], "git_dirty": state["git_dirty"], "git_tags": tags}


def capacity_scout_current_snapshot(root: Path, ledger: Path, batch_started_epoch: float) -> dict:
    telemetry_available = True
    try:
        temperature = int(Path("/sys/class/thermal/thermal_zone0/temp").read_text())
        throttle_text = subprocess.check_output(
            ["vcgencmd", "get_throttled"], text=True, stderr=subprocess.DEVNULL
        ).strip()
        throttled = throttle_text != "throttled=0x0"
    except (OSError, ValueError, subprocess.CalledProcessError):
        telemetry_available = False
        temperature = 0
        throttled = False
    largest_probe = 0
    for rate_dir in ledger.glob("**/rate-*"):
        if rate_dir.is_dir():
            largest_probe = max(
                largest_probe,
                sum(path.stat().st_size for path in rate_dir.rglob("*") if path.is_file()),
            )
    failures = []
    for status_path in ledger.rglob("canonical-status.json"):
        try:
            status = json.loads(status_path.read_text())
        except (OSError, ValueError):
            continue
        if status.get("status") == "failed":
            failures.append(str(status.get("detail", "unknown")))
    repeated = 0
    if failures:
        repeated = max(Counter(failures).values())
    batch_meta = json.loads((ledger / "batch.json").read_text())
    try:
        source = capacity_scout_source_state(root)
        provenance_matches = (
            source["git_sha"] == batch_meta["source_git_sha"]
            and source["git_dirty"] is False
            and source["git_tags"] == batch_meta["source_git_tags"]
        )
    except ValueError:
        provenance_matches = False
    return {
        "provenance_matches": provenance_matches,
        "telemetry_available": telemetry_available,
        "throttled": throttled,
        "temperature_millicelsius": temperature,
        "elapsed_secs": time.time() - batch_started_epoch,
        "free_bytes": shutil.disk_usage(ledger).free,
        "largest_probe_bytes": largest_probe,
        "repeated_systemic_failures": repeated,
    }


def wait_for_capacity_scout_safety(
    root: Path, ledger: Path, batch_started_epoch: float, item: RunItem, attempt: int
) -> dict:
    wait_started = time.monotonic()
    cooling = False
    cool_since = None
    while True:
        snapshot = capacity_scout_current_snapshot(root, ledger, batch_started_epoch)
        action = capacity_scout_safety_action(snapshot)
        write_capacity_scout_progress(
            ledger,
            f"safety-{action['action']}",
            item,
            attempt,
            snapshot["temperature_millicelsius"],
            snapshot["throttled"],
            error=action.get("reason"),
        )
        if action["action"] == "stop":
            return action
        if action["action"] == "pause":
            cooling = True
        if not cooling:
            return action
        if snapshot["temperature_millicelsius"] < 65_000:
            cool_since = cool_since or time.monotonic()
            if time.monotonic() - cool_since >= 10 * 60:
                return {"action": "proceed"}
        else:
            cool_since = None
        if time.monotonic() - wait_started >= 30 * 60:
            stopped = {"action": "stop", "reason": "thermal-cooldown-timeout"}
            write_capacity_scout_progress(
                ledger,
                "safety-stop",
                item,
                attempt,
                snapshot["temperature_millicelsius"],
                snapshot["throttled"],
                error=stopped["reason"],
            )
            return stopped
        time.sleep(60)


def capacity_scout_failed_attempt_evidence(output: Path) -> dict:
    try:
        detail = str(json.loads((output / "canonical-status.json").read_text()).get("detail", ""))
    except (OSError, ValueError):
        detail = "missing or invalid canonical-status.json"
    thermal = None
    telemetry_path = output / "pi-telemetry.csv"
    if telemetry_path.is_file():
        try:
            thermal = _read_pi_thermal(telemetry_path)
        except (OSError, ValueError):
            thermal = None
    counters = None
    for name in ("capacity-scout.json", "publisher-summary.json"):
        path = output / name
        if path.is_file():
            try:
                counters = json.loads(path.read_text()).get("messages") or json.loads(path.read_text())
            except (OSError, ValueError):
                counters = None
            break
    return {"detail": detail, "thermal": thermal, "counters": counters}


def capacity_scout_failed_attempt_stop_reason(output: Path) -> str | None:
    evidence = capacity_scout_failed_attempt_evidence(output)
    detail = evidence["detail"]
    lowered = detail.lower()
    if "provenance" in lowered or "counter" in lowered or "reconcile" in lowered:
        return "provenance-or-counter-drift"
    if "capacity-scout" in lowered and any(
        marker in lowered for marker in ("differs", "unexpected", "checksum", "invalid", "requires", "must")
    ):
        return "invalid-capacity-scout-evidence"
    telemetry_path = output / "pi-telemetry.csv"
    if telemetry_path.is_file() and evidence["thermal"] is None:
        return "telemetry-invalid"
    thermal = evidence["thermal"]
    if thermal is not None:
        if thermal["throttled"]:
            return "throttling"
        if thermal["max_temperature_millicelsius"] >= 75_000:
            return "temperature-75c"
    return None


def run_capacity_scout_item_with_timeout(root: Path, batch_id: str, item: RunItem) -> bool:
    def timeout_handler(_signum: int, _frame: object) -> None:
        raise TimeoutError(f"capacity-scout attempt exceeded {CAPACITY_SCOUT_ATTEMPT_TIMEOUT_SECS}s")

    previous = signal.signal(signal.SIGALRM, timeout_handler)
    signal.alarm(CAPACITY_SCOUT_ATTEMPT_TIMEOUT_SECS)
    try:
        return run_item(root, batch_id, item)
    finally:
        signal.alarm(0)
        signal.signal(signal.SIGALRM, previous)


def main() -> int:
    parser = argparse.ArgumentParser(description="Run resumable canonical Pi 5 evaluations")
    parser.add_argument("--root", type=Path, default=Path(__file__).resolve().parents[3])
    parser.add_argument("--experiments", default="all")
    parser.add_argument("--focused", action="store_true")
    parser.add_argument("--capacity-scout", action="store_true")
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
        capacity_scout = args.capacity_scout
        if capacity_scout:
            if args.focused or args.experiments != "all":
                raise ValueError("--capacity-scout cannot be combined with --focused or --experiments")
            if args.seed != CAPACITY_SCOUT_SEED:
                raise ValueError(f"capacity-scout seed must be {CAPACITY_SCOUT_SEED}")
            if not args.batch_id:
                raise ValueError("--capacity-scout requires --batch-id")
            schedule = []
            experiments = {"capacity-scout"}
        elif args.focused:
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
    if capacity_scout:
        ledger = root / "eval/results/capacity-scout" / f"rpi5-{batch_id}"
        batch_path = ledger / "batch.json"
        if batch_path.is_file():
            batch = json.loads(batch_path.read_text())
        else:
            source = capacity_scout_source_state(root)
            if not args.dry_run and source["git_dirty"]:
                raise ValueError("capacity-scout source must be clean")
            if not args.dry_run and not source["git_tags"]:
                raise ValueError("capacity-scout source must be tagged")
            batch = {
                "schema_version": 1,
                "batch_class": "capacity-scout",
                "thesis_evidence": False,
                "batch_id": batch_id,
                "source_git_sha": source["git_sha"],
                "source_git_tags": source["git_tags"],
                "started_at": utc_now(),
                "started_at_epoch": time.time(),
            }
            if not args.dry_run:
                ledger.mkdir(parents=True, exist_ok=True)
                batch_path.write_text(json.dumps(batch, indent=2) + "\n")
        decisions, accepted = load_capacity_scout_replay(root, batch_id)
        outcome = replay_capacity_scout_decisions(decisions, accepted)
        if outcome["action"] == "stop":
            print(json.dumps(outcome, indent=2))
            if not args.dry_run:
                (ledger / "scout-complete.json").write_text(json.dumps(outcome, indent=2) + "\n")
            return 0
        decision = outcome["decision"]
        if outcome["action"] == "launch":
            previous_decision = (
                ledger / "decisions" / f"decision-{decision['decision_index'] - 1:04d}.json"
            )
            previous_sha256 = (
                hashlib.sha256(previous_decision.read_bytes()).hexdigest()
                if previous_decision.is_file()
                else None
            )
            decision = {
                **decision,
                "previous_decision_sha256": previous_sha256,
                "persisted_at": utc_now(),
            }
            decision_path = ledger / "decisions" / f"decision-{decision['decision_index']:04d}.json"
            if not args.dry_run:
                persist_capacity_scout_decision(decision_path, decision)
        schedule = [RunItem(**item) for item in decision["schedule"]]
        if outcome["action"] == "resume":
            pending = set(outcome["pending_result_keys"])
            schedule = [item for item in schedule if item.result_key in pending]
        print_plan(schedule, CAPACITY_SCOUT_SEED, batch_id)
        print(json.dumps({"action": outcome["action"], "decision": decision}, indent=2))
        if args.dry_run:
            return 0

        write_capacity_scout_progress(
            ledger, "decision-started", None, None, None, None,
            counters={"pending_runs": len(schedule)},
        )
        for item in schedule:
            condition_dir = ledger / item.condition
            selection = select_attempt(condition_dir, item.run_index)
            attempt = int(selection.path.name.rsplit("-attempt-", 1)[-1])
            safety = wait_for_capacity_scout_safety(
                root, ledger, float(batch["started_at_epoch"]), item, attempt
            )
            if safety["action"] == "stop":
                stopped = {"timestamp": utc_now(), "item": item.result_key, **safety}
                (ledger / "safety-stop.json").write_text(json.dumps(stopped, indent=2) + "\n")
                return 2
            write_capacity_scout_progress(
                ledger, "probe-started", item, attempt,
                None, None,
            )
            passed = run_capacity_scout_item_with_timeout(root, batch_id, item)
            accepted_path = find_passed_attempt(condition_dir, item.run_index) if passed else None
            result = (
                json.loads((accepted_path / "capacity-scout.json").read_text())
                if accepted_path is not None
                else None
            )
            failure = capacity_scout_failed_attempt_evidence(selection.path) if not passed else None
            thermal = result.get("thermal") if result else (failure or {}).get("thermal")
            write_capacity_scout_progress(
                ledger,
                "probe-finished" if passed else "probe-invalid",
                item,
                attempt,
                thermal.get("max_temperature_millicelsius") if thermal else None,
                thermal.get("throttled") if thermal else None,
                result.get("messages") if result else (failure or {}).get("counters"),
                None if passed else (failure or {}).get("detail"),
            )
            if not passed:
                stop_reason = capacity_scout_failed_attempt_stop_reason(selection.path)
                if stop_reason is not None:
                    stopped = {
                        "timestamp": utc_now(),
                        "item": item.result_key,
                        "attempt": attempt,
                        "action": "stop",
                        "reason": stop_reason,
                    }
                    (ledger / "safety-stop.json").write_text(
                        json.dumps(stopped, indent=2) + "\n"
                    )
                    return 2
                return 1
        write_capacity_scout_progress(
            ledger, "decision-finished", None, None, None, None,
            counters={"completed_runs": len(schedule)},
        )
        return 0

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
