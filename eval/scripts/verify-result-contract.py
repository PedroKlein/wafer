#!/usr/bin/env python3
"""Structural verifier for eval/RESULT-CONTRACT.md compliance.

Walks every leaf run directory under the given experiment directories:
- **Core artefacts** must exist in every leaf.
- **Per-experiment artefacts** are checked where the contract requires
  them; an artefact the contract marks optional may be absent.

Exit codes:
    0  every leaf under the given experiment dirs conforms. A leaf whose
       system under test failed a criterion it measures (an OUTCOME line)
       still conforms: the failure is data, not a contract violation.
    1  at least one leaf violates the contract.
    2  invocation error (bad args).

Usage:
    verify-result-contract.py <experiment-run-dir>...

Example:
    verify-result-contract.py \\
      eval/results/e-val-1/rpi5-validation-2026-10-01T12-00-00Z
"""

from __future__ import annotations

import argparse
import csv
import hashlib
import json
import math
import re
import shlex
import sys
import tomllib
from pathlib import Path

ANALYSIS_SRC = Path(__file__).resolve().parents[1] / "analysis" / "src" / "wafer_analysis"
if str(ANALYSIS_SRC) not in sys.path:
    sys.path.insert(0, str(ANALYSIS_SRC))

from attempts import (
    EKUIPER_UNIT_PROPERTIES,
    INCOMPLETE_RUN_REASONS,
    ekuiper_exit_code,
    ekuiper_rule_running,
    ekuiper_unit_restarted,
    sut_outcome_reasons,
)
from results_layout import resolve_alias_receipt, validate_alias_mapping

sys.path.insert(0, str(Path(__file__).resolve().parent / "lib"))
from interval_metrics import validate_interval_metrics
from latency_evidence import latency_evidence_violations
from provenance_match import provenance_mismatches
from host_profiles import host_profiles

# The split contract (RESULT-CONTRACT.md source of truth).
CORE_FILES = {"config.toml", "metadata.json", "stdout.log"}
CANONICAL_PI_FILES = {
    "measurement-window.json",
    "pi-telemetry.csv",
    "pmic-rails.csv",
    "power-boundary.json",
}
CANONICAL_MATRIX = Path(__file__).resolve().parents[1] / "canonical-matrix.json"
HOST_PROFILES = host_profiles(json.loads(CANONICAL_MATRIX.read_text()))
MQTT_DRAIN_GRACE_SECS = int(
    json.loads(CANONICAL_MATRIX.read_text())["final_campaign"]["mqtt_drain_grace_secs"]
)
TARGET_LOAD_EXPERIMENTS = frozenset({"e-perf-1", "e-perf-2", "e-perf-3"})
MQTT_STOP_TOLERANCE_NS = 500_000_000
LATENCY_HIGHEST_NS = 3_600_000_000_000
FINAL_CAPACITY_REPETITIONS = 30
FINAL_CAPACITY_MEASUREMENT_SECS = 60
FINAL_CAPACITY_SYSTEMS = frozenset({"mqtt-loopback", "native", "wafer", "ekuiper"})
CANDIDATE_SCALING_EXPERIMENTS = {
    "e-perf-payload-refinement",
    "e-perf-depth-extension",
}
DLQ_CONTAINMENT_EXPERIMENTS = {f"e-iso-{index}" for index in range(1, 7)}
CANDIDATE_SWAP_EXPERIMENTS = {
    "e-swap-independent-sessions",
    "e-swap-rollback-sessions",
}
EKUIPER_PROFILE_EXPERIMENT = "e-compare-ekuiper-profile"
EKUIPER_PROFILE_RATES = {1_000, 4_000, 8_000}
EKUIPER_PROFILE_STATES = {"profiled", "unprofiled-control"}
PAYLOAD_REFINEMENT_GRID = {
    "120b": 120,
    "1kb": 1_024,
    "8kb": 8_192,
    "10kb": 10_240,
    "16kb": 16_384,
    "32kb": 32_768,
    "64kb": 65_536,
    "100kb": 102_400,
    "128kb": 131_072,
    "256kb": 262_144,
}
DEPTH_EXTENSION_GRID = {1, 3, 5, 10, 20, 50}
PASS_THROUGH_PLUGIN = (
    "../../../plugins/pass-through/target/wasm32-wasip2/release/wafer_pass_through.wasm"
)

# Runtime-provenance keys populated by `eval/scripts/lib/write_metadata.py`
# when it merges `runtime-provenance.json` (emitted by wafer-runtime) into
# `metadata.json`. A non-canonical leaf without them gets a warning; under
# `--canonical` it is a violation.
MERGED_PROVENANCE_KEYS = (
    "wasmtime_version",
    "wafer_runtime_sha256",
    "wafer_plugin_hashes",
)

def find_leaf_dirs(root: Path) -> list[Path]:
    """Return every directory under *root* that contains a `config.toml`.

    Leaf-run detection: a run is a directory holding a config.toml. This
    handles both single-run experiments (E-Val-1/run-N) and multi-tier
    experiments (E-Perf-6/depth-N/run-NN).
    """
    return [p.parent for p in root.rglob("config.toml")]


def experiment_of(path: Path) -> str | None:
    """Extract the experiment id from a path like
    eval/results/e-perf-6/shakedown-macos-.../..."""
    for part in path.parts:
        if re.match(r"^e-[a-z0-9-]+$", part):
            return part
    return None


def check_publisher_summary(path: Path) -> list[str]:
    violations: list[str] = []
    value = _load_json(path, "publisher-summary.json", violations)
    if value is None:
        return violations
    required = {
        "schema_version", "intended", "rejected", "enqueued", "acked",
        "unacked_at_exit", "connects", "measurement_duration_ns", "deadline_misses",
    }
    violations.extend(
        f"publisher-summary.json missing field: {field}"
        for field in sorted(required - value.keys())
    )
    if required <= value.keys():
        try:
            counters = [
                int(value[field])
                for field in (
                    "intended", "rejected", "enqueued", "acked", "unacked_at_exit",
                    "connects", "deadline_misses",
                )
            ]
            if any(counter < 0 for counter in counters):
                violations.append("publisher-summary.json counters must be non-negative")
            if int(value["measurement_duration_ns"]) <= 0:
                violations.append("publisher-summary.json measurement duration must be positive")
            if int(value["intended"]) != int(value["rejected"]) + int(value["enqueued"]):
                violations.append("publisher-summary.json counters do not reconcile")
            if int(value["enqueued"]) != int(value["acked"]) + int(value["unacked_at_exit"]):
                violations.append("publisher-summary.json acknowledged counters do not reconcile")
            if int(value["connects"]) != 1:
                violations.append(
                    f"publisher-summary.json publisher connected {value['connects']} times"
                )
            if int(value["unacked_at_exit"]) != 0:
                violations.append(
                    f"publisher-summary.json {value['unacked_at_exit']} messages were never acknowledged"
                )
            if value.get("exit_reason", "duration") != "duration":
                violations.append(
                    f"publisher-summary.json run stopped early: {value['exit_reason']}"
                )
        except (TypeError, ValueError):
            violations.append("publisher-summary.json counters must be integers")
    return violations


def check_measurement_window(path: Path) -> list[str]:
    try:
        window = json.loads(path.read_text())
        violations = []
        if int(window["finished_ns"]) <= int(window["started_ns"]):
            violations.append("canonical measurement window is empty or reversed")
        violations.extend(latency_evidence_violations(window))
        return violations
    except (AttributeError, KeyError, OSError, TypeError, ValueError):
        return ["canonical measurement window is invalid"]


def check_subscriber_metadata(path: Path) -> list[str]:
    violations: list[str] = []
    value = _load_json(path, "subscriber-metadata.json", violations)
    if value is None:
        return violations
    required = {
        "started_at_ns", "ended_at_ns", "exit_reason", "git_sha", "host_tag",
        "sequence_end_exclusive", "ignored_sequence_count",
        "total_recorded", "total_messages", "parse_errors", "negative_latency_count",
        "above_highest_latency_count", "clock_steps", "latency_p50_ns", "latency_p95_ns", "latency_p99_ns",
        "histogram_lowest_ns", "histogram_highest_ns", "histogram_sig_digits", "sequence",
    }
    violations.extend(
        f"subscriber-metadata.json missing field: {field}"
        for field in sorted(required - value.keys())
    )
    if not required <= value.keys():
        return violations
    if value.get("status", "complete") != "complete":
        reasons = "; ".join(str(reason) for reason in value.get("partial_reasons", []))
        violations.append(f"subscriber-metadata.json run is {value['status']}: {reasons}")
    sequence = value["sequence"]
    sequence_fields = {"total_received", "total_gaps", "total_duplicates", "out_of_range"}
    if not isinstance(sequence, dict) or not sequence_fields <= sequence.keys():
        return violations + ["subscriber-metadata.json sequence summary is invalid"]
    try:
        if int(value["ended_at_ns"]) <= int(value["started_at_ns"]):
            violations.append("subscriber-metadata.json measurement interval is invalid")
        if int(value["total_recorded"]) != int(sequence["total_received"]):
            violations.append("subscriber-metadata.json recorded count does not reconcile")
        if int(sequence["out_of_range"]) != 0:
            violations.append("subscriber-metadata.json sequence.out_of_range must be zero")
        if int(value["total_messages"]) != int(value["total_recorded"]):
            violations.append("subscriber-metadata.json message and HDR populations differ")
        if (
            int(value["histogram_lowest_ns"]) != 1_000
            or int(value["histogram_highest_ns"]) != LATENCY_HIGHEST_NS
            or int(value["histogram_sig_digits"]) != 3
        ):
            violations.append("subscriber-metadata.json histogram precision differs from the frozen recorder")
        for field in (
            "parse_errors", "negative_latency_count", "above_highest_latency_count", "clock_steps",
        ):
            if int(value[field]) != 0:
                violations.append(f"subscriber-metadata.json {field} must be zero")
    except (TypeError, ValueError):
        violations.append("subscriber-metadata.json counters and timestamps must be integers")
    return violations


def check_declared_sequence_range(leaf: Path) -> list[str]:
    """The summary checks already report a missing or unreadable file."""
    violations: list[str] = []
    publisher = _load_json(leaf / "publisher-summary.json", "publisher-summary.json", [])
    subscriber = _load_json(leaf / "subscriber-metadata.json", "subscriber-metadata.json", [])
    if publisher is None or subscriber is None:
        return violations
    intended = publisher.get("intended")
    sequence = subscriber.get("sequence")
    if (
        type(intended) is not int
        or subscriber.get("sequence_end_exclusive") != intended
        or not isinstance(sequence, dict)
        or sequence.get("expected") != intended
    ):
        violations.append(
            "subscriber-metadata.json does not declare the publisher's measured sequence range"
        )
    return violations


def check_mqtt_run_end(leaf: Path) -> list[str]:
    """The run used the matrix drain grace and the subscriber stopped within it."""
    violations: list[str] = []
    intervals = _load_json(leaf / "interval-metrics.json", "interval-metrics.json", [])
    subscriber = _load_json(leaf / "subscriber-metadata.json", "subscriber-metadata.json", [])
    window = _load_json(leaf / "measurement-window.json", "measurement-window.json", [])
    if intervals is None or subscriber is None or window is None:
        return violations
    grace_ns = MQTT_DRAIN_GRACE_SECS * 1_000_000_000
    if intervals.get("drain_grace_ns") != grace_ns:
        violations.append(
            f"interval-metrics.json drain_grace_ns is {intervals.get('drain_grace_ns')}, "
            f"must be the matrix MQTT drain grace {grace_ns}"
        )
    ended_ns = subscriber.get("ended_at_ns")
    finished_ns = window.get("finished_ns")
    if (
        type(ended_ns) is not int
        or type(finished_ns) is not int
        or ended_ns > finished_ns + grace_ns + MQTT_STOP_TOLERANCE_NS
    ):
        violations.append(
            "subscriber stopped later than the MQTT drain grace after the publisher exit"
        )
    return violations


def check_capacity_run_result(
    path: Path,
    *,
    expected_experiment: str = "e-perf-10",
    expected_batch_class: str = "final-capacity",
    expected_thesis_evidence: bool = True,
    repetitions: int = FINAL_CAPACITY_REPETITIONS,
    measurement_secs: int = FINAL_CAPACITY_MEASUREMENT_SECS,
    require_n30_exclusion: bool = False,
    allowed_rates: set[int] | None = None,
    allowed_rates_by_system: dict[str, set[int]] | None = None,
    expected_evidence_class: str | None = None,
) -> list[str]:
    violations: list[str] = []
    value = _load_json(path, "capacity-run.json", violations)
    if value is None:
        return violations
    required = {
        "schema_version", "experiment", "system", "thesis_evidence", "rate_msg_s",
        "run_index", "measurement_duration_ns", "messages", "rates_msg_s", "loss_percent",
        "latency_ns", "latency_hdr", "resources", "thermal", "process_audit", "config",
        "loadgen_profile", "provenance", "controlled_factors", "traces",
    }
    violations.extend(
        f"capacity-run.json missing field: {field}"
        for field in sorted(required - value.keys())
    )
    messages = value.get("messages", {})
    message_fields = {
        "intended", "rejected", "enqueued", "acked", "received_events", "received_unique",
        "downstream_lost", "total_undelivered", "duplicates",
    }
    if not isinstance(messages, dict):
        violations.append("capacity-run.json messages must be an object")
    else:
        violations.extend(
            f"capacity-run.json messages missing field: {field}"
            for field in sorted(message_fields - messages.keys())
        )
        if message_fields <= messages.keys():
            try:
                if int(messages["intended"]) != int(messages["rejected"]) + int(messages["enqueued"]):
                    violations.append("capacity-run.json intended counters do not reconcile")
                if int(messages["enqueued"]) != int(messages["acked"]):
                    violations.append("capacity-run.json enqueued and acknowledged counters differ")
                if int(messages["acked"]) != int(messages["received_unique"]) + int(messages["downstream_lost"]):
                    violations.append("capacity-run.json acknowledged counters do not reconcile")
                if int(messages["received_events"]) != int(messages["received_unique"]) + int(messages["duplicates"]):
                    violations.append("capacity-run.json received counters do not reconcile")
                if int(messages["total_undelivered"]) != int(messages["rejected"]) + int(messages["downstream_lost"]):
                    violations.append("capacity-run.json undelivered counters do not reconcile")
            except (TypeError, ValueError):
                violations.append("capacity-run.json message counters must be integers")
    if value.get("experiment") != expected_experiment:
        violations.append(f"capacity-run.json experiment must be {expected_experiment}")
    if value.get("system") not in FINAL_CAPACITY_SYSTEMS:
        violations.append("capacity-run.json system is not a capacity-grid system")
    try:
        if not 1 <= int(value.get("run_index")) <= repetitions:
            violations.append("capacity-run.json run_index is outside the frozen repetitions")
        if int(value.get("measurement_duration_ns")) != measurement_secs * 1_000_000_000:
            violations.append("capacity-run.json measurement duration differs from the frozen matrix")
        rate = int(value.get("rate_msg_s"))
        if allowed_rates is not None and rate not in allowed_rates:
            violations.append("capacity-run.json rate is outside the experiment grid")
        if (
            allowed_rates_by_system is not None
            and rate not in allowed_rates_by_system.get(str(value.get("system")), set())
        ):
            violations.append("capacity-run.json rate is outside the system grid")
        if int(messages.get("intended")) != rate * measurement_secs:
            violations.append("capacity-run.json intended population differs from rate times duration")
    except (KeyError, TypeError, ValueError):
        violations.append("capacity-run.json frozen duration/population fields are invalid")
    if value.get("batch_class") != expected_batch_class:
        violations.append(f"capacity-run.json batch_class must be {expected_batch_class}")
    if value.get("thesis_evidence") is not expected_thesis_evidence:
        violations.append(
            "capacity-run.json must set "
            f"thesis_evidence={str(expected_thesis_evidence).lower()}"
        )
    if expected_evidence_class is not None and value.get("evidence_class") != expected_evidence_class:
        violations.append("capacity-run.json evidence_class is invalid")
    if require_n30_exclusion and value.get("n30_admitted") is not False:
        violations.append("capacity-run.json candidate must set n30_admitted=false")
    if value.get("traces") is not False:
        violations.append("capacity-run.json final capture must be trace-free")
    try:
        intended = int(messages["intended"])
        received_unique = int(messages["received_unique"])
        total_undelivered = int(messages["total_undelivered"])
        duration_secs = int(value["measurement_duration_ns"]) / 1_000_000_000
        expected_loss_percent = 100.0 * total_undelivered / intended if intended else 100.0
        expected_achieved = received_unique / duration_secs
        if not math.isclose(float(value.get("loss_percent")), expected_loss_percent):
            violations.append("capacity-run.json loss_percent differs from counters")
        if not math.isclose(float(value.get("rates_msg_s", {}).get("achieved")), expected_achieved):
            violations.append("capacity-run.json achieved rate differs from counters")
        achieved_ratio = value.get("rates_msg_s", {}).get("achieved_ratio")
        if achieved_ratio is not None and not math.isclose(
            float(achieved_ratio), received_unique / intended if intended else 0.0
        ):
            violations.append("capacity-run.json achieved ratio differs from counters")
    except (KeyError, TypeError, ValueError, ZeroDivisionError):
        violations.append("capacity-run.json derived rates are invalid")
    histogram = value.get("latency_hdr")
    if not isinstance(histogram, dict) or not {
        "path", "sha256", "samples", "lowest_ns", "highest_ns", "significant_digits"
    } <= histogram.keys():
        violations.append("capacity-run.json latency_hdr summary is invalid")
    elif (
        histogram["samples"] != messages.get("received_events")
        or histogram["lowest_ns"] != 1_000
        or histogram["highest_ns"] != LATENCY_HIGHEST_NS
        or histogram["significant_digits"] != 3
    ):
        violations.append("capacity-run.json latency_hdr does not match received events or precision")
    return violations


def check_capacity_artifact_reconciliation(leaf: Path) -> list[str]:
    violations: list[str] = []
    publisher = _load_json(leaf / "publisher-summary.json", "publisher-summary.json", violations)
    subscriber = _load_json(leaf / "subscriber-metadata.json", "subscriber-metadata.json", violations)
    capacity = _load_json(leaf / "capacity-run.json", "capacity-run.json", violations)
    if publisher is None or subscriber is None or capacity is None:
        return violations
    expected = {
        "intended": publisher.get("intended"),
        "rejected": publisher.get("rejected"),
        "enqueued": publisher.get("enqueued"),
        "acked": publisher.get("acked"),
        "received_events": subscriber.get("total_recorded"),
        "duplicates": subscriber.get("sequence", {}).get("total_duplicates"),
        "ignored_warmup": subscriber.get("ignored_sequence_count"),
    }
    for field, value in expected.items():
        if capacity.get("messages", {}).get(field) != value:
            violations.append(f"capacity-run.json {field} differs from bounded source summary")
    if capacity.get("measurement_duration_ns") != publisher.get("measurement_duration_ns"):
        violations.append("capacity-run.json duration differs from publisher-summary.json")
    for percentile in ("p50", "p95", "p99"):
        if capacity.get("latency_ns", {}).get(percentile) != subscriber.get(
            f"latency_{percentile}_ns"
        ):
            violations.append(
                f"capacity-run.json {percentile} differs from subscriber-metadata.json"
            )
    expected_receipts = {
        "latency_hdr": "latency.hdr",
        "process_audit": "process-audit.json",
        "config": "config.toml",
        "loadgen_profile": "loadgen-profile.toml",
        "provenance": "metadata.json",
    }
    for field, expected_name in expected_receipts.items():
        receipt = capacity.get(field)
        if not isinstance(receipt, dict) or receipt.get("path") != expected_name:
            violations.append(f"capacity-run.json {field} receipt path is invalid")
            continue
        artifact = leaf / expected_name
        if not artifact.is_file():
            violations.append(f"capacity-run.json {field} receipt path is missing")
            continue
        if hashlib.sha256(artifact.read_bytes()).hexdigest() != receipt.get("sha256"):
            violations.append(f"capacity-run.json {field} receipt checksum mismatch")
    return violations


def _check_swap4_bucket_series(
    buckets: object, *, count: int, start_ns: int, label: str
) -> tuple[list[str], dict[str, int]]:
    totals = {"received_unique": 0, "received_events": 0, "duplicates": 0}
    if not isinstance(buckets, list) or len(buckets) != count:
        return [f"throughput-buckets.json must contain {count} {label} buckets"], totals
    violations = []
    expected_start = start_ns
    for bucket in buckets:
        try:
            unique = int(bucket["received_unique"])
            events = int(bucket["received_events"])
            duplicates = int(bucket["duplicates"])
            valid = (
                int(bucket["start_offset_ns"]) == expected_start
                and int(bucket["end_offset_ns"]) == expected_start + 100_000_000
                and events == unique + duplicates
                and float(bucket["rate_msg_s"]) == unique * 10.0
            )
        except (KeyError, TypeError, ValueError):
            violations.append(f"throughput-buckets.json {label} bucket schema is invalid")
            continue
        if not valid:
            violations.append(
                f"throughput-buckets.json {label} buckets are not contiguous or reconciled"
            )
        totals["received_unique"] += unique
        totals["received_events"] += events
        totals["duplicates"] += duplicates
        expected_start += 100_000_000
    return violations, totals


def _check_swap4_throughput(value: dict) -> list[str]:
    violations = []
    if (
        value.get("schema_version") != 1
        or value.get("clock") != "unix-epoch-source-sink-alignment"
        or type(value.get("source_measurement_start_unix_ns")) is not int
        or value.get("origin_mismatch_events") != 0
        or value.get("missing_origin_events") != 0
        or value.get("bucket_width_ns") != 100_000_000
        or value.get("coverage_start_offset_ns") != 0
        or value.get("coverage_end_offset_ns") != 120_000_000_000
        or value.get("drain_coverage_start_offset_ns") != 120_000_000_000
        or value.get("drain_coverage_end_offset_ns") != 130_000_000_000
    ):
        violations.append("throughput-buckets.json E-Swap-4 source-origin clock is invalid")
    primary_buckets = value.get("primary_buckets")
    drain_buckets = value.get("drain_buckets")
    primary_errors, primary = _check_swap4_bucket_series(
        primary_buckets, count=1_200, start_ns=0, label="primary"
    )
    drain_errors, drain = _check_swap4_bucket_series(
        drain_buckets, count=100, start_ns=120_000_000_000, label="drain"
    )
    violations.extend(primary_errors + drain_errors)
    for prefix, totals in (("primary", primary), ("drain", drain)):
        for field, total in totals.items():
            if value.get(f"{prefix}_{field}") != total:
                violations.append(f"throughput-buckets.json {prefix} totals do not reconcile")
    if not isinstance(primary_buckets, list) or not isinstance(drain_buckets, list):
        return violations
    primary_last = value.get("primary_last_offset_ns")
    drain_first = value.get("drain_first_offset_ns")
    drain_last = value.get("drain_last_offset_ns")
    primary_nonempty = [
        index for index, bucket in enumerate(primary_buckets)
        if int(bucket.get("received_events", 0)) > 0
    ]
    if primary["received_events"] > 0 and (
        type(primary_last) is not int
        or not primary_nonempty
        or not primary_nonempty[-1] * 100_000_000
        <= primary_last
        < (primary_nonempty[-1] + 1) * 100_000_000
    ):
        violations.append("throughput-buckets.json primary last offset is invalid")
    if primary["received_events"] == 0 and primary_last is not None:
        violations.append("throughput-buckets.json primary last offset must be null")
    drain_nonempty = [
        index for index, bucket in enumerate(drain_buckets)
        if int(bucket.get("received_events", 0)) > 0
    ]
    if drain["received_events"] > 0 and (
        type(drain_first) is not int
        or type(drain_last) is not int
        or not drain_nonempty
        or not 120_000_000_000 + drain_nonempty[0] * 100_000_000
        <= drain_first
        < 120_000_000_000 + (drain_nonempty[0] + 1) * 100_000_000
        or not 120_000_000_000 + drain_nonempty[-1] * 100_000_000
        <= drain_last
        < 120_000_000_000 + (drain_nonempty[-1] + 1) * 100_000_000
        or drain_first > drain_last
    ):
        violations.append("throughput-buckets.json drain offsets are invalid")
    if drain["received_events"] == 0 and (drain_first is not None or drain_last is not None):
        violations.append("throughput-buckets.json drain offsets must be null")
    try:
        after = {
            "received_unique": int(value["after_drain_unique"]),
            "received_events": int(value["after_drain_events"]),
            "duplicates": int(value["after_drain_duplicates"]),
        }
        after_first = value["after_drain_first_offset_ns"]
        after_last = value["after_drain_last_offset_ns"]
        if after["received_events"] != after["received_unique"] + after["duplicates"]:
            violations.append("throughput-buckets.json after-drain totals do not reconcile")
        if after["received_events"] > 0 and (
            type(after_first) is not int
            or type(after_last) is not int
            or not 130_000_000_000 <= after_first <= after_last
        ):
            violations.append("throughput-buckets.json after-drain offsets are invalid")
        if after["received_events"] == 0 and (after_first is not None or after_last is not None):
            violations.append("throughput-buckets.json after-drain offsets must be null")
        if after["received_events"] != 0 or value.get("drain_right_censored") is not False:
            violations.append("throughput-buckets.json drain is right-censored")
        full = {field: primary[field] + drain[field] + after[field] for field in primary}
        if any(value.get(field) != total for field, total in full.items()):
            violations.append("throughput-buckets.json full-run totals do not reconcile")
        offsets = [offset for offset in (primary_last, drain_last, after_last) if offset is not None]
        maximum = int(value["max_arrival_offset_ns"])
        if not offsets or maximum != max(offsets):
            violations.append("throughput-buckets.json maximum arrival offset does not reconcile")
        if (maximum < 120_000_000_000) != (
            drain["received_events"] == 0 and after["received_events"] == 0
        ):
            violations.append("throughput-buckets.json arrival region is invalid")
    except (KeyError, TypeError, ValueError):
        violations.append("throughput-buckets.json drain evidence is invalid")
    return violations


def check_throughput_buckets(path: Path, expected_count: int | None = None) -> list[str]:
    violations: list[str] = []
    value = _load_json(path, "throughput-buckets.json", violations)
    if value is None:
        return violations
    if expected_count == 1_200:
        return _check_swap4_throughput(value)
    expected_clock = "unix-epoch" if expected_count == 200 else "monotonic"
    if value.get("clock") != expected_clock or value.get("bucket_width_ns") != 100_000_000:
        violations.append(
            f"throughput-buckets.json must use {expected_clock} aligned 100 ms buckets"
        )
    if expected_count == 200:
        try:
            measurement_start = int(value["measurement_start_timestamp_ns"])
            scheduled_timestamp = int(value["scheduled_event_timestamp_ns"])
            event_timestamp = int(value["event_timestamp_ns"])
            alignment_error = int(value["alignment_error_ns"])
            event_metadata_valid = (
                value.get("clock_purpose") == "cross-process-alignment"
                and value.get("scheduled_event_offset_ns") == 60_000_000_000
                and scheduled_timestamp == measurement_start + 60_000_000_000
                and int(value["event_offset_from_measurement_start_ns"])
                    == event_timestamp - measurement_start
                and alignment_error == event_timestamp - scheduled_timestamp
                and value.get("alignment_tolerance_ns") == 10_000_000
                and abs(alignment_error) <= 10_000_000
                and value.get("coverage_start_offset_ns") == -10_000_000_000
                and value.get("coverage_end_offset_ns") == 10_000_000_000
            )
        except (KeyError, TypeError, ValueError):
            event_metadata_valid = False
        if not event_metadata_valid:
            violations.append("throughput-buckets.json event placement or coverage is invalid")
    buckets = value.get("buckets")
    if not isinstance(buckets, list) or not buckets:
        return violations + ["throughput-buckets.json buckets must be non-empty"]
    if expected_count is not None and len(buckets) != expected_count:
        violations.append(f"throughput-buckets.json must contain {expected_count} buckets")
    previous_end = None
    totals = {"received_unique": 0, "received_events": 0, "duplicates": 0}
    for bucket in buckets:
        required = {"start_offset_ns", "end_offset_ns", "received_unique", "received_events", "duplicates", "rate_msg_s"}
        if not isinstance(bucket, dict) or not required <= bucket.keys():
            violations.append("throughput-buckets.json bucket schema is invalid")
            continue
        if previous_end is None and expected_count == 200 and int(bucket["start_offset_ns"]) != -10_000_000_000:
            violations.append("throughput-buckets.json does not start at -10 seconds")
        if int(bucket["end_offset_ns"]) - int(bucket["start_offset_ns"]) != 100_000_000:
            violations.append("throughput-buckets.json bucket width is inconsistent")
        if previous_end is not None and int(bucket["start_offset_ns"]) != previous_end:
            violations.append("throughput-buckets.json buckets are not contiguous")
        previous_end = int(bucket["end_offset_ns"])
        try:
            unique = int(bucket["received_unique"])
            events = int(bucket["received_events"])
            duplicates = int(bucket["duplicates"])
            if events != unique + duplicates or float(bucket["rate_msg_s"]) != unique * 10.0:
                violations.append("throughput-buckets.json bucket counters or rate do not reconcile")
            for field in totals:
                totals[field] += int(bucket[field])
        except (TypeError, ValueError):
            violations.append("throughput-buckets.json bucket counters are invalid")
    if expected_count == 200:
        if previous_end != 10_000_000_000:
            violations.append("throughput-buckets.json does not end at +10 seconds")
        for field, total in totals.items():
            if value.get(field) != total:
                violations.append(f"throughput-buckets.json {field} total does not reconcile")
    return violations


def check_fine_event_buckets(
    path: Path,
    canonical_path: Path,
    *,
    experiment: str,
    expected_event_timestamp_ns: int | None = None,
) -> list[str]:
    violations: list[str] = []
    value = _load_json(path, "throughput-buckets-10ms.json", violations)
    canonical = _load_json(canonical_path, "throughput-buckets.json", violations)
    if value is None or canonical is None:
        return violations
    if (
        value.get("schema_version") != 1
        or value.get("alignment") != "actual-t0"
        or value.get("clock_purpose") != "cross-process-alignment"
        or value.get("bucket_width_ns") != 10_000_000
        or value.get("bucket_count") != 400
        or value.get("coverage_start_offset_ns") != -2_000_000_000
        or value.get("coverage_end_offset_ns") != 2_000_000_000
        or value.get("parent_bucket_width_ns") != 100_000_000
        or value.get("parent_bucket_count") != 40
        or value.get("loss_accounting") != "canonical-sequence-and-primary-drain-only"
    ):
        violations.append("throughput-buckets-10ms.json contract differs from the frozen window")
    expected_clock = (
        "unix-epoch" if experiment == "e-swap-3" else "unix-epoch-source-sink-alignment"
    )
    if value.get("clock") != expected_clock:
        violations.append("throughput-buckets-10ms.json clock differs from the experiment")
    try:
        event_timestamp = int(value["event_timestamp_ns"])
        scheduled_timestamp = int(value["scheduled_event_timestamp_ns"])
        alignment_error = event_timestamp - scheduled_timestamp
        if (
            int(value["alignment_error_ns"]) != alignment_error
            or int(value["alignment_tolerance_ns"]) != 10_000_000
            or abs(alignment_error) > 10_000_000
            or (
                expected_event_timestamp_ns is not None
                and event_timestamp != expected_event_timestamp_ns
            )
        ):
            violations.append("throughput-buckets-10ms.json actual-t0 alignment is invalid")
    except (KeyError, TypeError, ValueError):
        violations.append("throughput-buckets-10ms.json timing metadata is invalid")

    def validate_series(
        rows: object, *, count: int, width_ns: int, label: str
    ) -> tuple[dict[str, int], list[dict]]:
        totals = {"received_unique": 0, "received_events": 0, "duplicates": 0}
        if not isinstance(rows, list) or len(rows) != count:
            violations.append(
                f"throughput-buckets-10ms.json must contain {count} {label} buckets"
            )
            return totals, []
        expected_start = -2_000_000_000
        for row in rows:
            try:
                unique = int(row["received_unique"])
                events = int(row["received_events"])
                duplicates = int(row["duplicates"])
                if (
                    int(row["start_offset_ns"]) != expected_start
                    or int(row["end_offset_ns"]) != expected_start + width_ns
                    or events != unique + duplicates
                    or float(row["rate_msg_s"]) != unique * 1_000_000_000 / width_ns
                ):
                    violations.append(
                        f"throughput-buckets-10ms.json {label} buckets are not contiguous or reconciled"
                    )
                totals["received_unique"] += unique
                totals["received_events"] += events
                totals["duplicates"] += duplicates
                expected_start += width_ns
            except (KeyError, TypeError, ValueError):
                violations.append(
                    f"throughput-buckets-10ms.json {label} bucket schema is invalid"
                )
        if expected_start != 2_000_000_000:
            violations.append(
                f"throughput-buckets-10ms.json {label} coverage does not end at +2 seconds"
            )
        return totals, rows

    fine_totals, fine = validate_series(
        value.get("buckets"), count=400, width_ns=10_000_000, label="fine"
    )
    parent_totals, parents = validate_series(
        value.get("parent_buckets"), count=40, width_ns=100_000_000, label="parent"
    )
    if fine_totals != parent_totals:
        violations.append("throughput-buckets-10ms.json fine and parent totals differ")
    if fine and parents:
        for parent_index, parent in enumerate(parents):
            nested = fine[parent_index * 10 : (parent_index + 1) * 10]
            for field in ("received_unique", "received_events", "duplicates"):
                if int(parent[field]) != sum(int(row[field]) for row in nested):
                    violations.append(
                        "throughput-buckets-10ms.json fine buckets do not reconcile to parent population"
                    )
                    break
    for field, total in fine_totals.items():
        if value.get(field) != total:
            violations.append(f"throughput-buckets-10ms.json {field} total differs")
    if value.get("canonical_series") != "throughput-buckets.json":
        violations.append("throughput-buckets-10ms.json canonical series reference is invalid")
    if experiment == "e-swap-3":
        if value.get("event_timestamp_ns") != canonical.get("event_timestamp_ns"):
            violations.append("E-Swap-3 fine and canonical actual-t0 differ")
        canonical_rows = canonical.get("buckets")
        if isinstance(canonical_rows, list) and len(canonical_rows) == 200 and parents:
            for parent, canonical_parent in zip(parents, canonical_rows[80:120]):
                for field in ("received_unique", "received_events", "duplicates"):
                    if int(parent[field]) != int(canonical_parent[field]):
                        violations.append(
                            "E-Swap-3 fine buckets do not reconcile to canonical 100 ms population"
                        )
                        break
    elif experiment == "e-swap-4":
        try:
            source_origin = canonical.get("source_measurement_start_unix_ns")
            if (
                value.get("source_measurement_start_unix_ns") != source_origin
                or not isinstance(source_origin, int)
                or scheduled_timestamp != source_origin + 60_000_000_000
            ):
                violations.append("E-Swap-4 fine and canonical source origins differ")
            if any(
                fine_totals[field] > int(canonical[field])
                for field in ("received_unique", "received_events", "duplicates")
            ):
                violations.append("E-Swap-4 fine population exceeds canonical full-run population")
        except (KeyError, TypeError, ValueError):
            violations.append("E-Swap-4 fine/canonical population is invalid")
    else:
        violations.append("throughput-buckets-10ms.json is attached to an unsupported experiment")
    return violations


def check_disruption_timeline(path: Path) -> list[str]:
    violations: list[str] = []
    value = _load_json(path, "disruption-timeline.json", violations)
    if value is None:
        return violations
    required = {
        "timestamp_clock", "timestamp_clock_purpose", "scheduling_clock",
        "duration_clock", "strategy", "measurement_start_timestamp_ns",
        "scheduled_event_offset_ns", "scheduled_event_timestamp_ns",
        "event_timestamp_ns", "event_offset_from_measurement_start_ns",
        "alignment_error_ns", "alignment_tolerance_ns", "action_start_timestamp_ns",
        "action_end_timestamp_ns", "action_end_offset_ns", "action_start_monotonic_ns",
        "action_end_monotonic_ns", "action_duration_ns",
    }
    if not required <= value.keys():
        return violations + ["disruption-timeline.json is missing event fields"]
    measurement_start = int(value.get("measurement_start_timestamp_ns", -1))
    scheduled_timestamp = int(value.get("scheduled_event_timestamp_ns", -1))
    event_timestamp = int(value.get("event_timestamp_ns", -1))
    action_end_timestamp = int(value.get("action_end_timestamp_ns", -1))
    action_start_monotonic = int(value.get("action_start_monotonic_ns", -1))
    action_end_monotonic = int(value.get("action_end_monotonic_ns", -1))
    if (
        value["timestamp_clock"] != "unix-epoch"
        or value["timestamp_clock_purpose"] != "cross-process-alignment"
        or value["scheduling_clock"] != "monotonic"
        or value["duration_clock"] != "monotonic"
        or value["strategy"] not in {"wafer-hotswap", "wafer-restart", "ekuiper-restart"}
        or int(value["scheduled_event_offset_ns"]) != 60_000_000_000
        or scheduled_timestamp != measurement_start + 60_000_000_000
        or int(value["event_offset_from_measurement_start_ns"])
            != event_timestamp - measurement_start
        or int(value["alignment_error_ns"]) != event_timestamp - scheduled_timestamp
        or int(value["alignment_tolerance_ns"]) != 10_000_000
        or abs(int(value["alignment_error_ns"])) > 10_000_000
        or int(value["action_start_timestamp_ns"]) != event_timestamp
        or action_end_timestamp < event_timestamp
        or int(value["action_end_offset_ns"]) != action_end_timestamp - event_timestamp
        or action_end_monotonic < action_start_monotonic
        or int(value["action_duration_ns"])
            != action_end_monotonic - action_start_monotonic
    ):
        violations.append("disruption-timeline.json event/action clocks are invalid")
    return violations


def check_swap3_reconciliation(leaf: Path, metadata: dict) -> list[str]:
    violations: list[str] = []
    throughput = _load_json(leaf / "throughput-buckets.json", "throughput-buckets.json", violations)
    timeline = _load_json(leaf / "disruption-timeline.json", "disruption-timeline.json", violations)
    publisher = _load_json(leaf / "publisher-summary.json", "publisher-summary.json", violations)
    subscriber = _load_json(leaf / "subscriber-metadata.json", "subscriber-metadata.json", violations)
    if None in (throughput, timeline, publisher, subscriber):
        return violations
    try:
        matching_fields = (
            "measurement_start_timestamp_ns",
            "scheduled_event_timestamp_ns",
            "scheduled_event_offset_ns",
            "event_timestamp_ns",
            "event_offset_from_measurement_start_ns",
            "alignment_error_ns",
            "alignment_tolerance_ns",
        )
        if any(throughput.get(field) != timeline.get(field) for field in matching_fields):
            violations.append("E-Swap-3 bucket and action alignment metadata differ")
        if timeline["strategy"] != metadata.get("condition"):
            violations.append("E-Swap-3 strategy differs from metadata condition")
        intended = int(publisher["intended"])
        rejected = int(publisher["rejected"])
        enqueued = int(publisher["enqueued"])
        acked = int(publisher["acked"])
        received_events = int(subscriber["total_recorded"])
        duplicates = int(subscriber["sequence"]["total_duplicates"])
        received_unique = received_events - duplicates
        if intended != rejected + enqueued or acked > enqueued or received_unique > acked:
            violations.append("E-Swap-3 publisher/subscriber totals do not reconcile")
        if int(subscriber["sequence"]["total_received"]) != received_events:
            violations.append("E-Swap-3 subscriber sequence totals do not reconcile")
        if int(throughput["received_events"]) > received_events:
            violations.append("E-Swap-3 bucket population exceeds measured sequence population")
    except (KeyError, TypeError, ValueError):
        violations.append("E-Swap-3 bounded source summaries are invalid")
    return violations


def check_disruption_analysis(path: Path) -> list[str]:
    violations: list[str] = []
    value = _load_json(path, "disruption-analysis.json", violations)
    if value is None:
        return violations
    required = {
        "strategy", "baseline_rate_msg_s", "event_min_rate_msg_s", "dip_percent",
        "interruption_ns", "recovery_ns", "recovery_right_censored",
        "action_duration_ns", "loss", "duplicates", "messages", "latency_ns",
    }
    if not required <= value.keys():
        violations.append("disruption-analysis.json is missing estimator fields")
    if value.get("strategy") not in {"wafer-hotswap", "wafer-restart", "ekuiper-restart"}:
        violations.append("disruption-analysis.json strategy is invalid")
    for field in ("baseline_rate_msg_s", "event_min_rate_msg_s", "dip_percent"):
        if not isinstance(value.get(field), (int, float)) or value[field] < 0:
            violations.append(f"disruption-analysis.json {field} is invalid")
    for field in ("interruption_ns", "recovery_ns", "action_duration_ns", "loss", "duplicates"):
        if not isinstance(value.get(field), int) or value[field] < 0:
            violations.append(f"disruption-analysis.json {field} is invalid")
    if not isinstance(value.get("recovery_right_censored"), bool):
        violations.append("disruption-analysis.json recovery censoring is invalid")
    if set(value.get("latency_ns", {})) != {"p50", "p95", "p99"}:
        violations.append("disruption-analysis.json latency summary is invalid")
    return violations


def check_burst_timeline(path: Path) -> list[str]:
    violations: list[str] = []
    value = _load_json(path, "burst-timeline.json", violations)
    if value is None:
        return violations
    expected = {
        "before_rate_msg_s": 1_000,
        "burst_rate_msg_s": 2_000,
        "after_rate_msg_s": 1_000,
        "burst_start_offset_ns": 55_000_000_000,
        "scheduled_swap_offset_ns": 60_000_000_000,
        "burst_end_offset_ns": 65_000_000_000,
        "successful_swaps": 1,
    }
    for field, expected_value in expected.items():
        if value.get(field) != expected_value:
            violations.append(f"burst-timeline.json {field} must be {expected_value}")
    try:
        measurement_start = int(value["measurement_start_ns"])
        burst_start = int(value["burst_start_ns"])
        scheduled_swap = int(value["scheduled_swap_ns"])
        actual_swap = int(value["swap_ns"])
        burst_end = int(value["burst_end_ns"])
        if (
            value.get("timestamp_clock") != "unix-epoch"
            or value.get("timestamp_clock_purpose") != "cross-process-alignment"
            or value.get("scheduling_clock") != "monotonic"
            or burst_start != measurement_start + 55_000_000_000
            or scheduled_swap - burst_start != 5_000_000_000
            or burst_end - scheduled_swap != 5_000_000_000
            or int(value["actual_swap_offset_ns"]) != actual_swap - measurement_start
            or int(value["swap_alignment_error_ns"]) != actual_swap - scheduled_swap
            or int(value["swap_alignment_tolerance_ns"]) != 10_000_000
            or abs(actual_swap - scheduled_swap) > 10_000_000
            or not burst_start < actual_swap < burst_end
            or int(value["measurement_end_ns"]) <= burst_end
            or not 0 <= int(value["source_completion_offset_ns"]) < 130_000_000_000
        ):
            violations.append("burst-timeline.json clocks or centered swap are invalid")
    except (KeyError, TypeError, ValueError):
        violations.append("burst-timeline.json is missing timing evidence")
    phases = value.get("phases")
    expected_phases = {
        "before": (1_000, 0, 55_000_000_000, 55_000),
        "burst": (2_000, 55_000_000_000, 65_000_000_000, 20_000),
        "after": (1_000, 65_000_000_000, 120_000_000_000, 55_000),
    }
    if not isinstance(phases, dict) or set(phases) != set(expected_phases):
        return violations + ["burst-timeline.json phases are missing"]
    for name, (rate, start, end, intended) in expected_phases.items():
        phase = phases[name]
        if (
            phase.get("rate_msg_s") != rate
            or phase.get("start_offset_ns") != start
            or phase.get("end_offset_ns") != end
            or phase.get("intended") != intended
            or phase.get("emitted") != intended
            or not isinstance(phase.get("received"), int)
        ):
            violations.append(f"burst-timeline.json {name} phase is invalid")
    sequence = value.get("sequence")
    if not isinstance(sequence, dict) or set(sequence) != {
        "expected", "received", "gaps", "duplicates"
    }:
        violations.append("burst-timeline.json sequence evidence is invalid")
    elif (
        sequence["expected"] != 130_000
        or sum(phases[name]["received"] for name in expected_phases) != sequence["received"]
        or value.get("loss") != 130_000 - (sequence["received"] - sequence["duplicates"])
        or sequence["gaps"] != value.get("loss")
    ):
        violations.append("burst-timeline.json sequence totals do not reconcile")
    if (
        type(value.get("primary_received_events")) is not int
        or type(value.get("drain_received_events")) is not int
        or type(value.get("max_arrival_offset_ns")) is not int
        or value.get("drain_right_censored") is not False
    ):
        violations.append("burst-timeline.json drain evidence is invalid")
    drain_count = value.get("drain_received_events")
    drain_first = value.get("drain_first_offset_ns")
    drain_last = value.get("drain_last_offset_ns")
    if drain_count == 0 and (drain_first is not None or drain_last is not None):
        violations.append("burst-timeline.json drain offsets must be null")
    elif isinstance(drain_count, int) and drain_count > 0 and (
        type(drain_first) is not int
        or type(drain_last) is not int
        or not 120_000_000_000 <= drain_first <= drain_last < 130_000_000_000
        or value.get("drain_duration_after_window_ns") != drain_last - 120_000_000_000
    ):
        violations.append("burst-timeline.json drain offsets are invalid")
    internal_phases = value.get("internal_swap_phases_ns", {})
    if set(internal_phases) != {
        "compile_ns", "instantiate_ns", "signal_ns", "replacement_adopted_ns", "first_post_replacement_local_outcome_ns"
    } or any(type(duration) is not int or duration < 0 for duration in internal_phases.values()):
        violations.append("burst-timeline.json internal swap phases are invalid")
    return violations


def _check_swap4_actual_t0_receipt(
    receipt: dict,
    *,
    measurement_start_ns: int,
    scheduled_swap_ns: int,
    actual_swap_ns: int,
) -> list[str]:
    if (
        receipt.get("schema_version") != 1
        or receipt.get("clock") != "unix-epoch"
        or receipt.get("alignment") != "actual-t0"
        or receipt.get("source_measurement_start_unix_ns") != measurement_start_ns
        or receipt.get("scheduled_event_timestamp_ns") != scheduled_swap_ns
        or receipt.get("event_timestamp_ns") != actual_swap_ns
        or receipt.get("alignment_error_ns") != actual_swap_ns - scheduled_swap_ns
        or receipt.get("alignment_tolerance_ns") != 10_000_000
        or abs(actual_swap_ns - scheduled_swap_ns) > 10_000_000
    ):
        return ["swap-actual-t0.json does not reconcile with the declared swap timing"]
    return []



def check_swap4_reconciliation(leaf: Path) -> list[str]:
    violations: list[str] = []
    timeline = _load_json(leaf / "burst-timeline.json", "burst-timeline.json", violations)
    throughput = _load_json(
        leaf / "throughput-buckets.json", "throughput-buckets.json", violations
    )
    fine = _load_json(
        leaf / "throughput-buckets-10ms.json", "throughput-buckets-10ms.json", violations
    )
    sink_timeline = _load_json(leaf / "swap_timeline.json", "swap_timeline.json", violations)
    analysis = _load_json(leaf / "hotswap-analysis.json", "hotswap-analysis.json", violations)
    actual_t0 = _load_json(leaf / "swap-actual-t0.json", "swap-actual-t0.json", violations)
    source_timing = _load_json(
        leaf / "burst-source-timing.json", "burst-source-timing.json", violations
    )
    source_summary = _load_json(
        leaf / "burst-source-summary.json", "burst-source-summary.json", violations
    )
    try:
        requests = json.loads((leaf / "swap_requests.json").read_text())
    except (OSError, ValueError) as error:
        return violations + [f"swap_requests.json is unreadable: {error}"]
    if None in (timeline, throughput, fine, sink_timeline, analysis, actual_t0, source_timing, source_summary):
        return violations
    violations.extend(_check_swap4_throughput(throughput))
    try:
        transitions = sink_timeline["transitions"]
        events = analysis["events"]
        if (
            source_timing["measurement_start_ns"] != timeline["measurement_start_ns"]
            or source_summary["measurement_start_ns"] != timeline["measurement_start_ns"]
            or source_summary["source_completion_offset_ns"]
                != timeline["source_completion_offset_ns"]
        ):
            violations.append("E-Swap-4 source timing does not reconcile")
        if not isinstance(requests, list) or len(requests) != 1:
            violations.append("final E-Swap-4 must contain exactly one swap request")
        else:
            if timeline["swap_ns"] != requests[0]["request_started_ns"]:
                violations.append("E-Swap-4 actual swap timestamp differs from its request")
            else:
                violations.extend(
                    _check_swap4_actual_t0_receipt(
                        actual_t0,
                        measurement_start_ns=timeline["measurement_start_ns"],
                        scheduled_swap_ns=timeline["scheduled_swap_ns"],
                        actual_swap_ns=timeline["swap_ns"],
                    )
                )
        if len(transitions) != 1 or len(events) != 1 or analysis.get("sample_count") != 1:
            violations.append("final E-Swap-4 must contain exactly one swap event")
        elif (
            timeline["sink_observed_output_gap_ns"] != transitions[0]["pause_ns"]
            or timeline["sink_observed_output_gap_ns"]
                != events[0]["sink_observed_output_gap_ns"]
        ):
            violations.append("E-Swap-4 sink gap evidence does not reconcile")
        phase_received = [
            timeline["phases"][name]["received"] for name in ("before", "burst", "after")
        ]
        if phase_received != throughput["phase_received_messages"]:
            violations.append("E-Swap-4 phase receive populations do not reconcile")
        if throughput.get("source_measurement_start_unix_ns") != timeline.get(
            "measurement_start_ns"
        ):
            violations.append("E-Swap-4 source and sink origins do not reconcile")
        if (
            fine.get("source_measurement_start_unix_ns")
            != actual_t0.get("source_measurement_start_unix_ns")
            or fine.get("scheduled_event_timestamp_ns")
            != actual_t0.get("scheduled_event_timestamp_ns")
            or fine.get("event_timestamp_ns") != actual_t0.get("event_timestamp_ns")
        ):
            violations.append("E-Swap-4 actual-t0 receipt differs from the fine event buckets")
        if timeline["sequence"]["received"] != throughput["received_events"]:
            violations.append("E-Swap-4 sequence and bucket populations do not reconcile")
        if (
            timeline.get("primary_received_events") != throughput["primary_received_events"]
            or timeline.get("drain_received_events") != throughput["drain_received_events"]
            or timeline.get("drain_first_offset_ns") != throughput["drain_first_offset_ns"]
            or timeline.get("drain_last_offset_ns") != throughput["drain_last_offset_ns"]
            or timeline.get("max_arrival_offset_ns") != throughput["max_arrival_offset_ns"]
            or timeline.get("drain_right_censored") is not False
        ):
            violations.append("E-Swap-4 timeline drain evidence does not reconcile")
    except (KeyError, TypeError, ValueError):
        violations.append("E-Swap-4 cross-artifact evidence is invalid")
    return violations


def _load_json(path: Path, label: str, violations: list[str]) -> dict | None:
    try:
        value = json.loads(path.read_text())
    except (OSError, ValueError) as error:
        violations.append(f"{label} is unreadable: {error}")
        return None
    if not isinstance(value, dict):
        violations.append(f"{label} must contain an object")
        return None
    return value


def check_service_percentiles(leaf: Path) -> list[str]:
    violations: list[str] = []
    service = _load_json(leaf / "service-percentiles.json", "service-percentiles.json", violations)
    latency = _load_json(leaf / "percentiles.json", "percentiles.json", violations)
    if service is None or latency is None:
        return violations
    try:
        count = int(service["total_count"])
        if count <= 0 or count != int(latency["total_count"]):
            violations.append(
                f"service-percentiles.json counts {count} messages, "
                f"percentiles.json counts {latency['total_count']}"
            )
    except (KeyError, TypeError, ValueError):
        violations.append("service-percentiles.json or percentiles.json lacks total_count")
    return violations


def check_ekuiper_profile_artifacts(leaf: Path, metadata: dict) -> list[str]:
    violations: list[str] = []
    runtime = _load_json(
        leaf / "ekuiper-runtime-summary.json",
        "ekuiper-runtime-summary.json",
        violations,
    )
    overhead = _load_json(
        leaf / "profiler-overhead.json", "profiler-overhead.json", violations
    )
    if runtime is None or overhead is None:
        return violations
    condition = str(metadata.get("condition", ""))
    match = re.fullmatch(r"rate-(\d{5})/(profiled|unprofiled-control)", condition)
    if match is None:
        return [*violations, "eKuiper profile metadata condition is invalid"]
    rate = int(match.group(1))
    state = match.group(2)
    expected_identity = {
        "schema_version": 1,
        "experiment": EKUIPER_PROFILE_EXPERIMENT,
        "evidence_class": "diagnostic",
        "thesis_evidence": False,
        "n30_admitted": False,
        "sample_unit": "independent host run at one rate and profiler state",
        "condition": condition,
        "run_index": metadata.get("run_index"),
        "rate_msg_s": rate,
        "profiler_state": state,
        "source_git_sha": metadata.get("git_sha"),
        "source_dirty": metadata.get("git_dirty"),
        "measurement_source_leaf": metadata.get("measurement_source_leaf"),
        "shared_from": None,
        "claim_boundary": "diagnostic-association-only-not-gc-causality",
        "no_pool_with": ["e-perf-1", "e-perf-10", "prior diagnostic rehearsals"],
    }
    if any(runtime.get(field) != value for field, value in expected_identity.items()):
        violations.append("eKuiper runtime summary diagnostic identity is invalid")
    if rate not in EKUIPER_PROFILE_RATES or state not in EKUIPER_PROFILE_STATES:
        violations.append("eKuiper profile rate or state is outside the frozen grid")
    if (
        metadata.get("system") != "ekuiper"
        or metadata.get("batch_class") != "diagnostic-ekuiper-profile"
        or metadata.get("evidence_class") != "diagnostic"
        or metadata.get("thesis_evidence") is not False
        or metadata.get("n30_admitted") is not False
        or metadata.get("offered_rate_msg_s") != rate
        or metadata.get("shared_measurement") is not False
    ):
        violations.append("eKuiper profile metadata diagnostic identity is invalid")
    interval = runtime.get("interval_alignment", {})
    try:
        interval_path = leaf / str(interval["path"])
        if (
            interval.get("clock") != "unix-epoch"
            or interval.get("maximum_rows")
            != json.loads(interval_path.read_text()).get("maximum_rows")
            or not 1 <= int(interval["row_count"]) <= int(interval["maximum_rows"])
            or int(interval["measurement_end_ns"])
            - int(interval["measurement_start_ns"])
            != 60_000_000_000
            or hashlib.sha256(interval_path.read_bytes()).hexdigest()
            != interval.get("sha256")
        ):
            violations.append("eKuiper runtime summary interval alignment is invalid")
    except (KeyError, OSError, TypeError, ValueError):
        violations.append("eKuiper runtime summary interval alignment is invalid")
    gc_runtime = runtime.get("gc_runtime_metrics", {})
    gctrace_path = leaf / "ekuiper-gctrace.log"
    if gc_runtime.get("status") == "available":
        try:
            counts = [
                gc_runtime[field]
                for field in (
                    "trace_line_count",
                    "missing_cycle_count",
                    "cycle_count",
                    "stw_pause_total_ns",
                )
            ]
            if (
                state != "profiled"
                or gc_runtime.get("source") != "go-gctrace-journal"
                or gc_runtime.get("path") != gctrace_path.name
                or hashlib.sha256(gctrace_path.read_bytes()).hexdigest()
                != gc_runtime.get("sha256")
                or any(type(count) is not int or count < 0 for count in counts)
                or len(gctrace_path.read_text().splitlines())
                != gc_runtime["trace_line_count"]
                or gc_runtime["trace_line_count"] < 1
                or gc_runtime["cycle_count"] > gc_runtime["trace_line_count"]
                or (gc_runtime["stw_pause_max_ns"] is None)
                != (gc_runtime["cycle_count"] == 0)
            ):
                violations.append("eKuiper GC trace summary is invalid")
        except (KeyError, OSError, TypeError, ValueError):
            violations.append("eKuiper GC trace summary is invalid")
    elif (
        gc_runtime.get("status") != "unavailable"
        or not gc_runtime.get("reason")
        or (state == "profiled") == (gc_runtime["reason"] == "gctrace-disabled-by-design")
    ):
        violations.append("eKuiper GC trace availability is invalid")
    if state == "unprofiled-control" and gctrace_path.is_file() and gctrace_path.read_text():
        violations.append("eKuiper unprofiled control contains gctrace output")
    latency = runtime.get("latency_ns", {})
    try:
        interval_document = json.loads(interval_path.read_text())
        latency_count = int(latency["sample_count"])
        interval_count = int(interval_document["aggregate_latency_count"])
    except (KeyError, OSError, TypeError, ValueError):
        latency_count = -1
        interval_count = -2
    if (
        any(
            type(latency.get(field)) is not int or latency[field] < 0
            for field in ("sample_count", "p50", "p95", "p99")
        )
        or latency_count <= 0
        or latency_count != interval_count
    ):
        violations.append("eKuiper profile latency population does not reconcile")
    process = runtime.get("process_metrics", {})
    if process.get("status") == "available":
        path = leaf / str(process.get("path", ""))
        try:
            row_count = int(process["row_count"])
            if (
                state != "profiled"
                or row_count < 2
                or row_count > 62
                or process.get("maximum_rows") != 62
                or hashlib.sha256(path.read_bytes()).hexdigest()
                != process.get("sha256")
            ):
                violations.append("eKuiper process profile is invalid or unbounded")
        except (KeyError, OSError, TypeError, ValueError):
            violations.append("eKuiper process profile is invalid or unbounded")
    elif process.get("status") != "unavailable" or not process.get("reason"):
        violations.append("eKuiper process profile availability is invalid")
    expected_pair = (
        f"rate-{rate:05d}/unprofiled-control"
        if state == "profiled"
        else f"rate-{rate:05d}/profiled"
    )
    expected_role = (
        "sampler-enabled"
        if process.get("status") == "available"
        else "sampler-skipped-unavailable"
        if state == "profiled"
        else "unprofiled-control"
    )
    if (
        overhead.get("schema_version") != 1
        or overhead.get("experiment") != EKUIPER_PROFILE_EXPERIMENT
        or overhead.get("condition") != condition
        or overhead.get("run_index") != metadata.get("run_index")
        or overhead.get("rate_msg_s") != rate
        or overhead.get("profiler_state") != state
        or overhead.get("paired_condition") != expected_pair
        or overhead.get("pair_key") != f"rate-{rate:05d}/run-{metadata.get('run_index'):02d}"
        or overhead.get("overhead_estimator")
        != "paired-run-level-profiled-minus-unprofiled-control"
        or overhead.get("claim_boundary")
        != "diagnostic-association-only-not-gc-causality"
        or overhead.get("profile_collection_enabled")
        is not (process.get("status") == "available")
        or overhead.get("overhead_role") != expected_role
    ):
        violations.append("profiler-overhead.json pairing or claim boundary is invalid")
    if (
        state == "unprofiled-control"
        or process.get("status") == "unavailable"
    ) and (leaf / "resource-usage.csv").exists():
        violations.append("eKuiper leaf contains profiler output while collection is disabled")
    return violations


def check_ekuiper_godebug(leaf: Path, metadata: dict, experiment: str) -> list[str]:
    violations: list[str] = []
    audit = _load_json(leaf / "ekuiper-audit.json", "ekuiper-audit.json", violations)
    if audit is None:
        return violations
    environment = audit.get("service", {}).get("properties", {}).get("Environment", "")
    godebug = [
        entry for entry in shlex.split(str(environment)) if entry.startswith("GODEBUG=")
    ]
    profiled = experiment == EKUIPER_PROFILE_EXPERIMENT and str(
        metadata.get("condition", "")
    ).endswith("/profiled")
    if godebug != (["GODEBUG=gctrace=1"] if profiled else []):
        violations.append(f"eKuiper service GODEBUG {godebug} does not belong to this run")
    return violations


def check_ekuiper_health(leaf: Path, metadata: dict, experiment: str) -> list[str]:
    """The eKuiper snapshots must bracket the run and agree with the metadata and the audit.

    What they show about eKuiper is an outcome (``attempts.ekuiper_health_reasons``), not a
    violation; evidence the harness could not have written for this run is. Only E-Swap-3
    starts the rule again during the run, so elsewhere a new rule start time without a unit
    restart means something outside the run started it.
    """
    path = leaf / "ekuiper-health.json"
    if not path.is_file():
        return ["missing canonical eKuiper health artefact: ekuiper-health.json"]
    violations: list[str] = []
    health = _load_json(path, "ekuiper-health.json", violations)
    if health is None:
        return violations
    if (health.get("schema_version"), health.get("unit"), health.get("rule")) != (
        1,
        "kuiper.service",
        "pipeline_a",
    ):
        violations.append("ekuiper-health.json identity is invalid")
    for name in ("before", "after"):
        snapshot = health.get(name)
        service = snapshot.get("service") if isinstance(snapshot, dict) else None
        if (
            not isinstance(service, dict)
            or any(
                type(service.get(key)) is not int or service[key] < 0
                for key in EKUIPER_UNIT_PROPERTIES
            )
            or type(snapshot.get("captured_at_ns")) is not int
            or "rule_status" not in snapshot
        ):
            violations.append(f"ekuiper-health.json {name} snapshot is malformed")
    if violations:
        return violations
    before, after = health["before"], health["after"]
    if before["service"]["MainPID"] == 0 or not ekuiper_rule_running(before["rule_status"]):
        violations.append("eKuiper did not run pipeline_a before warm-up")
    try:
        window = json.loads((leaf / "measurement-window.json").read_text())
        started, finished = int(window["started_ns"]), int(window["finished_ns"])
    except (KeyError, OSError, TypeError, ValueError):
        pass
    else:
        if before["captured_at_ns"] > started or after["captured_at_ns"] < finished:
            violations.append("ekuiper-health.json snapshots do not bracket the measurement window")
    exit_codes = metadata.get("exit_codes")
    if (
        not isinstance(exit_codes, dict)
        or "ekuiper" not in exit_codes
        or exit_codes["ekuiper"] != ekuiper_exit_code(health)
    ):
        violations.append("metadata exit_codes.ekuiper differs from ekuiper-health.json")
    rule_before, rule_after = before["rule_status"], after["rule_status"]
    if (
        not ekuiper_unit_restarted(health)
        and isinstance(rule_before, dict)
        and isinstance(rule_after, dict)
    ):
        started_again = rule_after.get("lastStartTimestamp") != rule_before.get(
            "lastStartTimestamp"
        )
        if experiment == "e-swap-3" and not started_again:
            violations.append("E-Swap-3 restarted the eKuiper rule but its start time did not move")
        elif experiment != "e-swap-3" and started_again:
            violations.append("something outside the run started the eKuiper rule again")
    audit_path = leaf / "ekuiper-audit.json"
    if audit_path.is_file():
        audit = _load_json(audit_path, "ekuiper-audit.json", [])
        audited_pid = (audit or {}).get("service", {}).get("properties", {}).get("MainPID")
        if audit is not None and str(audited_pid) != str(before["service"]["MainPID"]):
            violations.append(
                "ekuiper-audit.json and ekuiper-health.json name different eKuiper main PIDs"
            )
    return violations


def check_candidate_swap_evidence(leaf: Path, metadata: dict, experiment: str) -> list[str]:
    violations: list[str] = []
    artifact_name = (
        "hotswap-analysis.json"
        if experiment == "e-swap-independent-sessions"
        else "rollback.json"
    )
    value = _load_json(leaf / artifact_name, artifact_name, violations)
    if value is None:
        return violations
    expected = {
        "e-swap-independent-sessions": {
            "batch_class": "candidate-independent-swap",
            "condition": "steady",
            "nested_unit": "swap event within run",
            "no_pool_with": [
                "e-swap-1",
                "e-swap-2",
                "e-swap-6",
                "prior diagnostic rehearsals",
            ],
        },
        "e-swap-rollback-sessions": {
            "batch_class": "candidate-rollback-session",
            "condition": "process-trap-rollback",
            "nested_unit": "rollback event within run",
            "no_pool_with": ["e-swap-5", "prior diagnostic rehearsals"],
        },
    }[experiment]
    identity = {
        "schema_version": 1,
        "batch_class": expected["batch_class"],
        "experiment": experiment,
        "condition": expected["condition"],
        "evidence_class": "candidate-supplementary",
        "thesis_evidence": False,
        "n30_admitted": False,
        "sample_unit": "independent host run",
        "nested_unit": expected["nested_unit"],
        "event_classes": ["first-use-aot", "cached"],
        "no_pool_with": expected["no_pool_with"],
        "duration_unit": "ns",
        "sample_count": 50,
    }
    if any(value.get(field) != expected_value for field, expected_value in identity.items()):
        violations.append(f"{artifact_name} candidate identity is invalid")
    if (
        value.get("run_index") != metadata.get("run_index")
        or value.get("measurement_source_leaf") != metadata.get("measurement_source_leaf")
        or value.get("shared_from") is not None
    ):
        violations.append(f"{artifact_name} does not reconcile with metadata.json")
    events = value.get("events")
    if not isinstance(events, list) or len(events) != 50:
        return violations + [f"{artifact_name} must contain exactly 50 events"]
    if [event.get("event_index") for event in events] != list(range(50)):
        violations.append(f"{artifact_name} event indices must be exactly 0 through 49")
    if [event.get("event_class") for event in events] != ["first-use-aot", *("cached" for _ in range(49))]:
        violations.append(f"{artifact_name} first-use and cached event labels are invalid")
    sequence = value.get("sequence")
    try:
        with (leaf / "sequence.csv").open(newline="") as stream:
            rows = list(csv.DictReader(stream))
        if len(rows) != 1:
            raise ValueError
        expected_sequence = {
            "expected": int(rows[0]["total_expected"]),
            "received": int(rows[0]["total_received"]),
            "gaps": int(rows[0]["gap_msgs"]),
            "duplicates": int(rows[0]["duplicates_count"]),
        }
        if sequence != expected_sequence:
            raise ValueError
    except (KeyError, OSError, TypeError, ValueError):
        violations.append(f"{artifact_name} sequence evidence does not reconcile")
    required = (
        {"compile_ns", "instantiate_ns", "signal_ns", "replacement_adopted_ns", "first_post_replacement_local_outcome_ns", "http_total_ns", "sink_observed_output_gap_ns"}
        if experiment == "e-swap-independent-sessions"
        else {"compile_ns", "instantiate_ns", "signal_ns", "rollback_ns", "http_total_ns"}
    )
    if any(
        any(type(event.get(field)) is not int or event[field] < 0 for field in required)
        for event in events
    ):
        violations.append(f"{artifact_name} event durations are invalid")
    try:
        requests = json.loads((leaf / "swap_requests.json").read_text())
        if not isinstance(requests, list) or len(requests) != 50:
            raise ValueError
        for event, request in zip(events, requests, strict=True):
            event_index = event["event_index"]
            expected_plugin = (
                "wafer_pass_through_v2_panics.wasm"
                if experiment == "e-swap-rollback-sessions"
                else "wafer_pass_through_v2.wasm"
                if event_index % 2 == 0
                else "wafer_pass_through_v1.wasm"
            )
            if event.get("plugin") != expected_plugin or request.get("plugin") != expected_plugin:
                raise ValueError
            timeline = request["body"]["timeline"]
            fields = {"compile_ns", "instantiate_ns", "signal_ns"}
            if experiment == "e-swap-independent-sessions":
                fields |= {"replacement_adopted_ns", "first_post_replacement_local_outcome_ns"}
                if event["sink_observed_output_gap_ns"] != json.loads(
                    (leaf / "swap_timeline.json").read_text()
                )["transitions"][event["event_index"]]["pause_ns"]:
                    raise ValueError
            else:
                fields.add("rollback_ns")
                if request.get("http_status") != 200 or request["body"].get("status") != "rolled_back":
                    raise ValueError
            if any(event[field] != timeline[field] for field in fields):
                raise ValueError
            if event["http_total_ns"] != request["request_duration_ns"]:
                raise ValueError
    except (KeyError, OSError, TypeError, ValueError):
        counterpart = (
            "swap requests/timeline"
            if experiment == "e-swap-independent-sessions"
            else "swap requests"
        )
        violations.append(f"{artifact_name} does not reconcile with {counterpart}")
    if experiment == "e-swap-rollback-sessions" and (
        value.get("attempts") != 50
        or value.get("rolled_back") != 50
        or value.get("all_rolled_back") is not True
    ):
        violations.append("rollback.json must show fifty successful rollbacks")
    return violations


def check_payload_manifest(path: Path, metadata: dict) -> list[str]:
    violations: list[str] = []
    value = _load_json(path, "payload-manifest.json", violations)
    if value is None:
        return violations
    expected_identity = {
        "schema_version": 1,
        "batch_class": "candidate-payload-refinement",
        "experiment": "e-perf-payload-refinement",
        "evidence_class": "candidate-supplementary",
        "thesis_evidence": False,
        "n30_admitted": False,
        "sample_unit": "independent host run at one payload size",
        "source_kind": "bench-source",
        "source_pattern": "repeated-byte-0x42",
        "transform_plugin_path": PASS_THROUGH_PLUGIN,
        "sink_kind": "bench-sink",
        "no_pool_with": ["e-perf-4", "prior diagnostic rehearsals"],
    }
    if any(value.get(field) != expected for field, expected in expected_identity.items()):
        violations.append("payload-manifest.json candidate identity is invalid")
    condition = value.get("condition")
    payload_bytes = value.get("payload_bytes")
    if condition not in PAYLOAD_REFINEMENT_GRID or payload_bytes != PAYLOAD_REFINEMENT_GRID[condition]:
        violations.append("payload-manifest.json condition and byte count differ")
    elif value.get("payload_sha256") != hashlib.sha256(b"B" * payload_bytes).hexdigest():
        violations.append("payload-manifest.json payload checksum differs")
    config_path = path.with_name("config.toml")
    try:
        config = tomllib.loads(config_path.read_text())
        nodes = config["nodes"]
        edges = config["edges"]
        source = nodes["source"]
        transforms = [node for node in nodes.values() if node.get("type") == "transform"]
        sinks = [
            node
            for node in nodes.values()
            if node.get("type") == "sink" and node.get("kind") == "bench-sink"
        ]
        if (
            source.get("kind") != "bench-source"
            or source.get("payload_size") != payload_bytes
            or source.get("rate") != 1_000.0
            or source.get("warmup_messages") != 30_000
            or source.get("total_messages") != 90_000
            or len(nodes) != 3
            or len(transforms) != 1
            or transforms[0].get("plugin") != PASS_THROUGH_PLUGIN
            or len(sinks) != 1
            or edges != [
                {"from": "source", "to": "transform"},
                {"from": "transform", "to": "sink"},
            ]
            or value.get("source_node") != "source"
            or value.get("transform_node") != "transform"
            or value.get("sink_node") != "sink"
            or value.get("edges") != edges
            or value.get("config_sha256") != hashlib.sha256(config_path.read_bytes()).hexdigest()
        ):
            violations.append("payload-manifest.json does not reconcile with config.toml")
    except (KeyError, OSError, TypeError, ValueError, tomllib.TOMLDecodeError):
        violations.append("payload-manifest.json cannot reconcile with config.toml")
    if value.get("run_index") != metadata.get("run_index") or condition != metadata.get("condition"):
        violations.append("payload-manifest.json does not reconcile with metadata.json")
    if value.get("run_index") not in range(1, 6):
        violations.append("payload-manifest.json run index is outside N=5")
    if (
        value.get("rate_msg_s") != 1_000
        or value.get("warmup_messages") != 30_000
        or value.get("measurement_messages") != 60_000
        or value.get("total_messages") != 90_000
    ):
        violations.append("payload-manifest.json run population differs from the candidate contract")
    return violations


def check_topology_manifest(path: Path, metadata: dict) -> list[str]:
    violations: list[str] = []
    value = _load_json(path, "topology-manifest.json", violations)
    if value is None:
        return violations
    expected_identity = {
        "schema_version": 1,
        "batch_class": "candidate-depth-extension",
        "experiment": "e-perf-depth-extension",
        "evidence_class": "candidate-supplementary",
        "thesis_evidence": False,
        "n30_admitted": False,
        "sample_unit": "independent host run at one pipeline depth",
        "no_pool_with": [
            "e-perf-3",
            "e-perf-6",
            "e-perf-8",
            "prior diagnostic rehearsals",
        ],
    }
    if any(value.get(field) != expected for field, expected in expected_identity.items()):
        violations.append("topology-manifest.json candidate identity is invalid")
    depth = value.get("depth")
    if depth not in DEPTH_EXTENSION_GRID or value.get("condition") != f"depth-{depth}":
        violations.append("topology-manifest.json depth is outside the frozen grid")
    config_path = path.with_name("config.toml")
    try:
        config = tomllib.loads(config_path.read_text())
        nodes = config["nodes"]
        edges = config["edges"]
        transforms = [node for node in nodes.values() if node.get("type") == "transform"]
        plugin_paths = {node.get("plugin") for node in transforms}
        expected_edges = [{"from": "source", "to": "t1"}]
        expected_edges.extend(
            {"from": f"t{index}", "to": f"t{index + 1}"}
            for index in range(1, depth)
        )
        expected_edges.append({"from": f"t{depth}", "to": "sink"})
        if (
            len(nodes) != depth + 2
            or len(transforms) != depth
            or len(edges) != depth + 1
            or edges != expected_edges
            or plugin_paths != {PASS_THROUGH_PLUGIN}
            or nodes.get("source", {}).get("kind") != "bench-source"
            or nodes.get("sink", {}).get("kind") != "bench-sink"
            or value.get("node_count") != len(nodes)
            or value.get("source_count") != 1
            or value.get("source_kind") != "bench-source"
            or value.get("sink_count") != 1
            or value.get("sink_kind") != "bench-sink"
            or value.get("transform_count") != len(transforms)
            or value.get("edge_count") != len(edges)
            or value.get("node_ids") != list(nodes)
            or value.get("edges") != expected_edges
            or value.get("transform_plugin_paths") != sorted(plugin_paths)
            or value.get("identical_transform_behavior") is not True
            or value.get("engine_fuel_budgets")
            != {"transform": 10_000_000, "filter": 500_000, "router": 500_000}
            or value.get("epoch_deadline") != 100
            or value.get("epoch_tick_ms") != 10
            or value.get("effective_metering_mode") != "fuel-and-epoch"
            or value.get("config_sha256") != hashlib.sha256(config_path.read_bytes()).hexdigest()
        ):
            violations.append("topology-manifest.json does not reconcile with config.toml")
    except (KeyError, OSError, TypeError, ValueError, tomllib.TOMLDecodeError):
        violations.append("topology-manifest.json cannot reconcile with config.toml")
    if value.get("run_index") != metadata.get("run_index") or value.get("condition") != metadata.get("condition"):
        violations.append("topology-manifest.json does not reconcile with metadata.json")
    if value.get("run_index") not in range(1, 6):
        violations.append("topology-manifest.json run index is outside N=5")
    if (
        value.get("payload_bytes") != 128
        or value.get("rate_msg_s") != 1_000
        or value.get("warmup_messages") != 30_000
        or value.get("measurement_messages") != 60_000
    ):
        violations.append("topology-manifest.json source workload differs from the candidate contract")
    return violations


CONTAINER_FLOOR_PLATFORMS = {"aarch64": "linux/arm64", "x86_64": "linux/amd64"}


def check_container_floor(path: Path, metadata: dict) -> list[str]:
    violations: list[str] = []
    value = _load_json(path, "container-floor.json", violations)
    if value is None:
        return violations
    if value.get("schema_version") != 1 or value.get("base") != "scratch" or value.get("layer_count") != 1:
        violations.append("container-floor.json is not a one-layer FROM scratch image")
    image_bytes = value.get("image_bytes")
    binary_bytes = value.get("binary_bytes")
    if not (
        type(image_bytes) is int and type(binary_bytes) is int and 0 < binary_bytes <= image_bytes
    ):
        violations.append("container-floor.json sizes must be positive with the worker inside the image")
    if value.get("platform") != CONTAINER_FLOOR_PLATFORMS.get(str(metadata.get("arch"))):
        violations.append("container-floor.json platform differs from the host architecture")
    if not re.fullmatch(r"rust:\S+-alpine@sha256:[0-9a-f]{64}", str(value.get("build_image", ""))):
        violations.append("container-floor.json build image is not pinned by digest")
    inputs = value.get("inputs_sha256")
    if not isinstance(inputs, dict) or not inputs or not all(
        re.fullmatch(r"[0-9a-f]{64}", str(digest)) for digest in inputs.values()
    ):
        violations.append("container-floor.json lacks the hashes of its build inputs")
    return violations


def expected_metering(matrix: dict, experiment: str, condition: str) -> dict:
    if experiment == "e-perf-7":
        values = matrix["experiments"][experiment]["metering_modes"][condition]
    else:
        definition = matrix["experiments"].get(experiment)
        if definition is None:
            definition = matrix.get("enhanced_candidate", {}).get("experiments", {}).get(experiment, {})
        values = definition.get("metering_exceptions", {}).get(
            condition, matrix["final_campaign"]["canonical_metering"]
        )
    fuel = values.get("fuel")
    budgets = (
        {"transform": fuel, "filter": None, "router": None}
        if not isinstance(fuel, dict)
        else fuel
    )
    epoch = values.get("epoch_deadline")
    has_fuel = any(value is not None for value in budgets.values())
    mode = {
        (False, False): "neither",
        (True, False): "fuel-only",
        (False, True): "epoch-only",
        (True, True): "fuel-and-epoch",
    }[(has_fuel, epoch is not None)]
    return {
        "engine_fuel_budgets": budgets,
        "epoch_deadline": epoch,
        "epoch_tick_ms": values.get("epoch_tick_ms", 10),
        "effective_metering_mode": mode,
    }


def batch_host(leaf: Path) -> str | None:
    """The host named by the nearest `<host>-` directory of a leaf, if any.

    Canonical leaves sit under a `<host>-<batch>` directory; a single-run
    leaf from run-experiment.sh is itself named `<host>-<timestamp>`.
    """
    for part in reversed(leaf.parts):
        for tag in HOST_PROFILES:
            if part.startswith(f"{tag}-"):
                return tag
    return None


def host_telemetry_violations(leaf: Path, profile) -> list[str]:
    violations = []
    boundary_path = leaf / "power-boundary.json"
    if boundary_path.is_file():
        try:
            measurement = json.loads(boundary_path.read_text()).get("measurement")
        except (OSError, ValueError, AttributeError) as exc:
            return [f"power-boundary.json unreadable: {exc}"]
        if measurement != profile.power_measurement:
            violations.append(
                f"{profile.tag} power measurement is {measurement!r}, expected {profile.power_measurement!r}"
            )
    telemetry_path = leaf / "pi-telemetry.csv"
    if telemetry_path.is_file():
        try:
            with telemetry_path.open(newline="") as stream:
                states = {row.get("throttled") for row in csv.DictReader(stream)}
        except (OSError, csv.Error) as exc:
            return [*violations, f"pi-telemetry.csv unreadable: {exc}"]
        throttled = sorted(str(state) for state in states - {profile.throttled})
        if throttled:
            violations.append(f"host telemetry recorded throttling during the run: {throttled}")
    return violations


def check_leaf(
    leaf: Path,
    experiment: str,
    canonical: bool = False,
    canonical_matrix: dict | None = None,
) -> tuple[list[str], list[str]]:
    """Return (violations, warnings). Empty lists = fully conformant.

    A failed criterion of the system under test is an outcome, not a violation. When the
    system stopped the run early (it exited, or a swap or rollback failed), only the
    artefacts the harness owns are checked: core files, provenance, host and telemetry.
    """
    files = {f.name for f in leaf.iterdir() if f.is_file()}
    violations: list[str] = []
    warnings: list[str] = []
    outcomes = sut_outcome_reasons(leaf, experiment)
    incomplete = bool(INCOMPLETE_RUN_REASONS & set(outcomes))

    for core in CORE_FILES:
        if core in files:
            continue
        violations.append(f"missing core artefact: {core}")

    for export_errors in sorted(leaf.glob("**/export-errors.json")):
        violations.append(
            f"{export_errors.relative_to(leaf)} present: BenchSink could not write every artifact"
        )

    meta_path = leaf / "metadata.json"
    metadata: dict = {}
    if meta_path.is_file():
        try:
            with meta_path.open() as fh:
                metadata = json.load(fh)
            if "diagnostic_repetitions" in metadata:
                warnings.append(
                    f"diagnostic batch run with {metadata['diagnostic_repetitions']} "
                    "repetitions: not thesis evidence"
                )
            missing = [k for k in MERGED_PROVENANCE_KEYS if k not in metadata]
            if missing and metadata.get("system") not in {"ekuiper", "mqtt-loopback", "static"}:
                message = (
                    f"metadata.json lacks merged provenance keys: {sorted(missing)} "
                    f"(written by eval/scripts/lib/write_metadata.py)"
                )
                if canonical:
                    violations.append(message)
                else:
                    warnings.append(message)
            directory_host = batch_host(leaf) if canonical else batch_host(Path(leaf.name))
            if directory_host is not None or canonical:
                profile = HOST_PROFILES[
                    directory_host
                    or (metadata.get("host_tag") if metadata.get("host_tag") in HOST_PROFILES else "rpi5")
                ]
                violations.extend(
                    f"{profile.tag} metadata: {error}" for error in profile.fact_errors(metadata)
                )
                if canonical:
                    violations.extend(host_telemetry_violations(leaf, profile))
                if not re.fullmatch(r"[0-9a-f]{40}", str(metadata.get("git_sha", ""))):
                    violations.append("Pi 5 metadata lacks a source commit SHA")
                if not isinstance(metadata.get("git_dirty"), bool):
                    violations.append("Pi 5 metadata git_dirty is not boolean")
                exit_codes = metadata.get("exit_codes", {})
                if metadata.get("system") == "ekuiper":
                    if exit_codes.get("ekuiper") != 0 and "runtime-exit" not in outcomes:
                        violations.append("Pi 5 metadata records a non-zero eKuiper exit")
                elif metadata.get("system") == "mqtt-loopback":
                    if exit_codes.get("publisher") != 0 or exit_codes.get("subscriber") != 0:
                        violations.append("Pi 5 metadata records a non-zero loopback loadgen exit")
                elif metadata.get("system") == "static":
                    if exit_codes.get("collector") != 0:
                        violations.append("Pi 5 metadata records a non-zero static collector exit")
                elif exit_codes.get("wafer_runtime") != 0 and "runtime-exit" not in outcomes:
                    violations.append("Pi 5 metadata records a non-zero runtime exit")
                diagnostic = "diagnostic_repetitions" in metadata
                if diagnostic and metadata.get("thesis_evidence") is not False:
                    violations.append("diagnostic metadata must set thesis_evidence=false")
                if experiment == "e-perf-10":
                    expected_evidence = not diagnostic
                    if metadata.get("thesis_evidence") is not expected_evidence:
                        violations.append(
                            f"E-Perf-10 metadata must set thesis_evidence={str(expected_evidence).lower()}"
                        )
                if experiment == "e-perf-capacity-knee":
                    if metadata.get("thesis_evidence") is not False:
                        violations.append("capacity-knee metadata must set thesis_evidence=false")
                    if metadata.get("n30_admitted") is not False:
                        violations.append("capacity-knee metadata must set n30_admitted=false")
                    if metadata.get("evidence_class") != "candidate-supplementary":
                        violations.append("capacity-knee metadata evidence_class is invalid")
                if experiment in CANDIDATE_SCALING_EXPERIMENTS | CANDIDATE_SWAP_EXPERIMENTS:
                    expected_batch_class = {
                        "e-perf-payload-refinement": "candidate-payload-refinement",
                        "e-perf-depth-extension": "candidate-depth-extension",
                        "e-swap-independent-sessions": "candidate-independent-swap",
                        "e-swap-rollback-sessions": "candidate-rollback-session",
                    }[experiment]
                    if (
                        metadata.get("experiment") != experiment
                        or metadata.get("thesis_evidence") is not False
                        or metadata.get("n30_admitted") is not False
                        or metadata.get("evidence_class") != "candidate-supplementary"
                        or metadata.get("batch_class") != expected_batch_class
                    ):
                        violations.append(f"{experiment} metadata candidate identity is invalid")
                if experiment == EKUIPER_PROFILE_EXPERIMENT:
                    if (
                        metadata.get("experiment") != experiment
                        or metadata.get("system") != "ekuiper"
                        or metadata.get("thesis_evidence") is not False
                        or metadata.get("n30_admitted") is not False
                        or metadata.get("evidence_class") != "diagnostic"
                        or metadata.get("batch_class") != "diagnostic-ekuiper-profile"
                        or metadata.get("shared_measurement") is not False
                    ):
                        violations.append(
                            "eKuiper profile metadata diagnostic identity is invalid"
                        )
                if canonical:
                    if metadata.get("system") == "wafer" and canonical_matrix is not None:
                        expected = expected_metering(
                            canonical_matrix, experiment, str(metadata.get("condition", ""))
                        )
                        actual = {key: metadata.get(key) for key in expected}
                        if actual != expected:
                            violations.append(
                                f"metering provenance differs from matrix/condition: {actual!r} != {expected!r}"
                            )
                    if metadata.get("git_dirty") is not False:
                        violations.append("canonical result records dirty source")
                    if (
                        metadata.get("system") not in {"ekuiper", "mqtt-loopback", "static"}
                        and "runtime-provenance.json" not in files
                    ):
                        violations.append("missing canonical runtime provenance: runtime-provenance.json")
        except (OSError, ValueError) as exc:
            warnings.append(f"metadata.json unreadable: {exc}")

    if canonical:
        for required in CANONICAL_PI_FILES:
            if required not in files:
                violations.append(f"missing canonical Pi telemetry artefact: {required}")
        window_path = leaf / "measurement-window.json"
        if window_path.is_file():
            violations.extend(check_measurement_window(window_path))
        if metadata.get("system") == "ekuiper":
            violations.extend(check_ekuiper_health(leaf, metadata, experiment))

    if incomplete:
        return violations, warnings

    if canonical and canonical_matrix is not None:
        experiment_contract = (
            canonical_matrix.get("experiments", {}).get(experiment)
            or canonical_matrix.get("enhanced_candidate", {}).get("experiments", {}).get(experiment)
        )
        if not isinstance(experiment_contract, dict):
            violations.append(f"experiment {experiment} is absent from canonical matrix")
        else:
            required_outputs = experiment_contract.get("required_outputs", [])
            for required in required_outputs:
                if required not in files:
                    violations.append(
                        f"missing required canonical artefact for {experiment}: {required}"
                    )
            interval_contract = (
                canonical_matrix.get("enhanced_candidate", {})
                .get("instrumentation", {})
                .get("bounded_interval_metrics", {})
            )
            timed_output = int(experiment_contract.get("measurement_secs", 0)) > 0 and bool(
                {"latency.hdr", "throughput.csv"} & set(required_outputs)
            )
            if interval_contract and timed_output:
                for required in ("interval-latency.json", "interval-metrics.json"):
                    if required not in files:
                        violations.append(
                            f"missing bounded interval artefact for {experiment}: {required}"
                        )
    for fragment_path in leaf.rglob("interval-latency.json"):
        interval_path = fragment_path.with_name("interval-metrics.json")
        if not interval_path.is_file():
            violations.append(
                f"timed interval fragment lacks composed interval-metrics.json: {fragment_path.parent}"
            )
            continue
        aggregate_count = None
        subscriber_path = fragment_path.with_name("subscriber-metadata.json")
        percentiles_path = fragment_path.with_name("percentiles.json")
        try:
            if subscriber_path.is_file():
                aggregate_count = int(json.loads(subscriber_path.read_text())["total_recorded"])
            elif percentiles_path.is_file():
                aggregate_count = int(json.loads(percentiles_path.read_text())["total_count"])
            validate_interval_metrics(interval_path, aggregate_count)
        except (KeyError, OSError, TypeError, ValueError) as error:
            violations.append(f"invalid interval-metrics.json: {error}")

    if experiment == "e-perf-payload-refinement" and "payload-manifest.json" in files:
        violations.extend(check_payload_manifest(leaf / "payload-manifest.json", metadata))
    if experiment == "e-perf-depth-extension" and "topology-manifest.json" in files:
        violations.extend(check_topology_manifest(leaf / "topology-manifest.json", metadata))
    if "service-percentiles.json" in files:
        violations.extend(check_service_percentiles(leaf))
    if experiment == "e-density-1" and "container-floor.json" in files:
        violations.extend(check_container_floor(leaf / "container-floor.json", metadata))
    if experiment in CANDIDATE_SWAP_EXPERIMENTS:
        violations.extend(check_candidate_swap_evidence(leaf, metadata, experiment))
    if experiment == EKUIPER_PROFILE_EXPERIMENT:
        violations.extend(check_ekuiper_profile_artifacts(leaf, metadata))
    if "ekuiper-audit.json" in files:
        violations.extend(check_ekuiper_godebug(leaf, metadata, experiment))
    if experiment in DLQ_CONTAINMENT_EXPERIMENTS and "containment.json" in files:
        containment = _load_json(leaf / "containment.json", "containment.json", violations)
        if containment is not None:
            violations.extend(check_dlq_evidence(leaf, containment))
    if experiment == "e-perf-10":
        for historical in ("rate-sweep.json", "published.csv", "received.csv"):
            if historical in files:
                violations.append(f"final E-Perf-10 must not contain historical trace artifact: {historical}")
    if canonical and experiment in TARGET_LOAD_EXPERIMENTS:
        violations.extend(check_publisher_summary(leaf / "publisher-summary.json"))
        violations.extend(check_subscriber_metadata(leaf / "subscriber-metadata.json"))
        violations.extend(check_declared_sequence_range(leaf))
        violations.extend(check_mqtt_run_end(leaf))
    if experiment == "e-perf-10" and "capacity-run.json" in files:
        violations.extend(check_publisher_summary(leaf / "publisher-summary.json"))
        violations.extend(check_subscriber_metadata(leaf / "subscriber-metadata.json"))
        violations.extend(check_mqtt_run_end(leaf))
        violations.extend(check_capacity_run_result(leaf / "capacity-run.json"))
        violations.extend(check_capacity_artifact_reconciliation(leaf))
    if experiment == "e-perf-capacity-knee" and "capacity-run.json" in files:
        matrix = canonical_matrix
        if matrix is None:
            try:
                matrix = json.loads(CANONICAL_MATRIX.read_text())
            except (OSError, ValueError) as error:
                violations.append(f"cannot load capacity-knee contract: {error}")
        definition = (
            matrix.get("enhanced_candidate", {}).get("experiments", {}).get(experiment)
            if isinstance(matrix, dict)
            else None
        )
        violations.extend(check_publisher_summary(leaf / "publisher-summary.json"))
        violations.extend(check_subscriber_metadata(leaf / "subscriber-metadata.json"))
        if not isinstance(definition, dict):
            violations.append("capacity-knee experiment is absent from the matrix")
        else:
            violations.extend(check_capacity_run_result(
                leaf / "capacity-run.json",
                expected_experiment=experiment,
                expected_batch_class="candidate-capacity-knee",
                expected_thesis_evidence=False,
                repetitions=int(definition["repetitions"]),
                measurement_secs=int(definition["measurement_secs"]),
                require_n30_exclusion=True,
                allowed_rates_by_system={
                    system: {int(rate) for rate in rates}
                    for system, rates in definition["condition_grid_msg_s"].items()
                },
                expected_evidence_class="candidate-supplementary",
            ))
        violations.extend(check_capacity_artifact_reconciliation(leaf))
    if experiment == "e-swap-3":
        if "publisher-timing.json" in files:
            violations.append("final E-Swap-3 must not retain publisher-timing.json")
        if "swap_timeline.json" in files:
            violations.append("final E-Swap-3 must not retain legacy swap_timeline.json")
        violations.extend(check_throughput_buckets(leaf / "throughput-buckets.json", 200))
        violations.extend(
            check_fine_event_buckets(
                leaf / "throughput-buckets-10ms.json",
                leaf / "throughput-buckets.json",
                experiment=experiment,
            )
        )
        violations.extend(check_disruption_timeline(leaf / "disruption-timeline.json"))
        violations.extend(check_publisher_summary(leaf / "publisher-summary.json"))
        violations.extend(check_subscriber_metadata(leaf / "subscriber-metadata.json"))
        violations.extend(check_declared_sequence_range(leaf))
        violations.extend(check_mqtt_run_end(leaf))
        violations.extend(check_disruption_analysis(leaf / "disruption-analysis.json"))
        violations.extend(check_swap3_reconciliation(leaf, metadata))
    if experiment == "e-swap-4":
        violations.extend(check_burst_timeline(leaf / "burst-timeline.json"))
        violations.extend(check_swap4_reconciliation(leaf))
        violations.extend(
            check_fine_event_buckets(
                leaf / "throughput-buckets-10ms.json",
                leaf / "throughput-buckets.json",
                experiment=experiment,
            )
        )
        analysis = _load_json(leaf / "hotswap-analysis.json", "hotswap-analysis.json", violations)
        if analysis is not None and (
            analysis.get("sample_count") != 1
            or not isinstance(analysis.get("events"), list)
            or len(analysis["events"]) != 1
        ):
            violations.append("final E-Swap-4 must contain exactly one swap event")

    return violations, warnings


def check_dlq_evidence(leaf: Path, containment: dict) -> list[str]:
    """The dead-letter file must hold one record per message the nodes sent it."""
    dlq_path = leaf / "dlq.jsonl"
    if not dlq_path.is_file():
        return []
    try:
        with dlq_path.open(encoding="utf-8") as stream:
            records = sum(1 for line in stream if line.strip())
        sent = sum(int(node.get("dlq_sent", 0)) for node in containment.get("nodes", []))
    except (OSError, TypeError, ValueError, AttributeError):
        return ["dlq.jsonl or containment dlq_sent counters are unreadable"]
    if records != sent:
        return [f"dlq.jsonl holds {records} records but nodes sent {sent} to the dead-letter queue"]
    return []


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__.split("\n\n", 1)[0])
    parser.add_argument(
        "--canonical",
        action="store_true",
        help="enforce Pi 5 provenance and experiment-specific canonical outputs",
    )
    parser.add_argument("--matrix", type=Path, default=CANONICAL_MATRIX)
    parser.add_argument(
        "--match",
        action="append",
        default=[],
        type=Path,
        metavar="DIR",
        help="also require the leaves under DIR (another host's batch, say) to share "
        "the source SHA and plugin hashes of the verified leaves, without checking "
        "their contract",
    )
    parser.add_argument("dirs", nargs="+", type=Path)
    args = parser.parse_args()

    canonical_matrix: dict | None = None
    if args.canonical:
        try:
            canonical_matrix = json.loads(args.matrix.read_bytes())
        except (OSError, ValueError) as exc:
            print(f"error: cannot load canonical matrix: {exc}", file=sys.stderr)
            return 2

    all_violations: list[tuple[Path, str]] = []
    all_warnings: list[tuple[Path, str]] = []
    all_outcomes: list[tuple[Path, str]] = []
    provenance_leaves: list[Path] = []
    checked = 0

    for root in args.dirs:
        if not root.exists():
            print(f"warn: {root} does not exist", file=sys.stderr)
            continue
        alias_receipts = (
            [root]
            if root.is_file()
            else sorted(root.rglob("run-*.json"))
            if "aliases" in root.parts
            else []
        )
        if alias_receipts:
            alias_targets: list[tuple[Path, str]] = []
            for receipt_path in alias_receipts:
                try:
                    receipt, source = resolve_alias_receipt(receipt_path)
                    source_experiment = receipt.get("shared_from_experiment")
                    if not isinstance(source_experiment, str):
                        raise ValueError("alias source experiment is missing")
                    alias_targets.append((source, source_experiment))
                except ValueError as error:
                    all_violations.append((receipt_path, str(error)))
            roots = alias_targets
        else:
            experiment = experiment_of(root)
            if experiment is None:
                print(f"warn: cannot infer experiment id from {root}", file=sys.stderr)
                continue
            roots = [(leaf, experiment) for leaf in find_leaf_dirs(root)]
            if not roots:
                print(f"warn: no leaf run dirs (config.toml) under {root}", file=sys.stderr)
                continue
        for leaf, experiment in roots:
            checked += 1
            provenance_leaves.append(leaf)
            violations, warnings = check_leaf(
                leaf,
                experiment,
                canonical=args.canonical,
                canonical_matrix=canonical_matrix,
            )
            for v in violations:
                all_violations.append((leaf, v))
            for w in warnings:
                all_warnings.append((leaf, w))
            outcomes = sut_outcome_reasons(leaf, experiment)
            if outcomes:
                all_outcomes.append((leaf, ", ".join(outcomes)))

    matched_leaves: list[Path] = []
    for other in args.match:
        found = sorted(find_leaf_dirs(other)) if other.is_dir() else []
        if not found:
            all_violations.append((other, "--match directory has no leaf run dirs"))
        matched_leaves.extend(found)
    if args.canonical or matched_leaves:
        all_violations.extend(
            provenance_mismatches(sorted(provenance_leaves) + matched_leaves)
        )

    for leaf, w in all_warnings:
        print(f"WARN       {leaf}: {w}")
    for leaf, reasons in all_outcomes:
        print(f"OUTCOME    {leaf}: system under test failed {reasons}")

    if all_violations:
        for leaf, v in all_violations:
            print(f"VIOLATION  {leaf}: {v}")
        print(f"\n{len(all_violations)} violation(s), {len(all_warnings)} warning(s) across {checked} leaf run(s).")
        return 1

    warn_count = len(all_warnings)
    if warn_count:
        print(f"OK: {checked} leaf run(s) conform to split RESULT-CONTRACT ({warn_count} warning(s) — non-fatal)")
    else:
        print(f"OK: {checked} leaf run(s) conform to split RESULT-CONTRACT")
    return 0


if __name__ == "__main__":
    sys.exit(main())
