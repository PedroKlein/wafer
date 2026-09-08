#!/usr/bin/env python3
"""Structural verifier for eval/RESULT-CONTRACT.md compliance.

Confirms the F3 split-contract decision (option B) works in practice:
- **Core artefacts** MUST exist in every leaf run directory.
- **Optional artefacts** may exist per the per-experiment matrix; when
  absent, this is contract-conformant, not a violation.

Reviewer note: this replaces the AC F3.AC5 "run two fresh shakedowns"
requirement with structural inspection of the existing post-decision
shakedown dirs. Cheaper and reproducible; the reviewer's real question
is 'does the split-contract hold?' which is best answered by walking
the manifest.

Exit codes:
    0  every leaf under the given experiment dirs conforms.
    1  at least one leaf violates core or optional contract in a way
       the split-contract permits (e.g. contract says memory.csv MUST
       exist for E-Perf-6 but it is absent).
    2  invocation error (bad args).

Usage:
    verify-result-contract.py <experiment-run-dir>...

Example:
    verify-result-contract.py \\
      eval/results/e-val-1/shakedown-macos-2026-07-22T16-19-29Z \\
      eval/results/e-perf-6/shakedown-macos-2026-07-22T17-49-56Z
"""

from __future__ import annotations

import argparse
import csv
import hashlib
import json
import math
import re
import sys
import tomllib
from pathlib import Path

ANALYSIS_SRC = Path(__file__).resolve().parents[1] / "analysis" / "src" / "wafer_analysis"
if str(ANALYSIS_SRC) not in sys.path:
    sys.path.insert(0, str(ANALYSIS_SRC))

from results_layout import resolve_alias_receipt

sys.path.insert(0, str(Path(__file__).resolve().parent / "lib"))
from interval_metrics import validate_interval_metrics

# The split contract (RESULT-CONTRACT.md source of truth).
CORE_FILES = {"config.toml", "metadata.json", "stdout.log"}
CANONICAL_PI_FILES = {
    "measurement-window.json",
    "pi-telemetry.csv",
    "pmic-rails.csv",
    "power-boundary.json",
}
CANONICAL_MATRIX = Path(__file__).resolve().parents[1] / "canonical-matrix.json"
FINAL_CAPACITY_REPETITIONS = 30
FINAL_CAPACITY_MEASUREMENT_SECS = 60
CANDIDATE_SCALING_EXPERIMENTS = {
    "e-perf-payload-refinement",
    "e-perf-depth-extension",
}
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
# `metadata.json`. T8 (thesis-hardening): shakedown dirs lacking these keys
# get a WARN, not a violation — legacy shakedown scripts write bespoke
# minimal metadata by design (see script header comments). Canonical
# `run-experiment.sh` runs always merge these keys.
MERGED_PROVENANCE_KEYS = (
    "wasmtime_version",
    "wafer_runtime_sha256",
    "wafer_plugin_hashes",
)

# Optional-artefact expectations per experiment. Match on the experiment
# prefix embedded in the shakedown path. `must_have`: files whose absence
# is a violation for THIS experiment. `may_have`: files legitimately
# missing under the split contract; presence is fine, absence is fine.
OPTIONAL_MATRIX: dict[str, dict[str, set[str]]] = {
    "e-val-1": {
        "must_have": {"latency.hdr", "throughput.csv"},
        "may_have": {"sequence.csv", "percentiles.json"},
        "must_not_have": set(),  # memory.csv/per_node_metrics.csv absent by design
    },
    "e-perf-6": {
        "must_have": {"latency.hdr", "throughput.csv", "memory.csv"},
        "may_have": {"sequence.csv", "per_node_metrics.csv", "metadata.json"},
        "must_not_have": set(),
    },
}

# Legacy shakedown scripts that predate F2 metadata merge and F3 split
# contract still write bespoke leaf layouts. Track them as documented
# deviations so the verifier doesn't flag known-unshipped gaps as
# regressions.
LEGACY_METADATA_MISSING_ALLOWED = {"e-val-1"}


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


def check_rate_sweep_result(path: Path) -> list[str]:
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
    nested = {
        "messages": {"offered", "received", "lost", "duplicates"},
        "latency_ns": {"p50", "p95", "p99"},
        "resources": {"scope", "cpu_percent", "max_rss_bytes"},
        "profile": {"path", "sha256", "payload_template_sha256"},
        "process_audit": {"path", "sha256"},
        "traces": {"published", "received"},
    }
    try:
        result = json.loads(path.read_text())
    except (OSError, ValueError) as error:
        return [f"rate-sweep.json is unreadable: {error}"]
    violations = [
        f"rate-sweep.json missing field: {field}"
        for field in sorted(required - result.keys())
    ]
    for section, fields in nested.items():
        value = result.get(section)
        if not isinstance(value, dict):
            violations.append(f"rate-sweep.json {section} must be an object")
            continue
        violations.extend(
            f"rate-sweep.json {section} missing field: {field}"
            for field in sorted(fields - value.keys())
        )
    if result.get("experiment") != "e-perf-10":
        violations.append("rate-sweep.json experiment must be e-perf-10")
    if result.get("thesis_evidence") is not False:
        violations.append("rate-sweep.json must set thesis_evidence=false")
    return violations


def check_publisher_summary(path: Path) -> list[str]:
    violations: list[str] = []
    value = _load_json(path, "publisher-summary.json", violations)
    if value is None:
        return violations
    required = {
        "schema_version", "intended", "rejected", "enqueued",
        "measurement_duration_ns", "deadline_misses",
    }
    violations.extend(
        f"publisher-summary.json missing field: {field}"
        for field in sorted(required - value.keys())
    )
    if required <= value.keys():
        try:
            counters = [
                int(value[field])
                for field in ("intended", "rejected", "enqueued", "deadline_misses")
            ]
            if any(counter < 0 for counter in counters):
                violations.append("publisher-summary.json counters must be non-negative")
            if int(value["measurement_duration_ns"]) <= 0:
                violations.append("publisher-summary.json measurement duration must be positive")
            if int(value["intended"]) != int(value["rejected"]) + int(value["enqueued"]):
                violations.append("publisher-summary.json counters do not reconcile")
        except (TypeError, ValueError):
            violations.append("publisher-summary.json counters must be integers")
    return violations


def check_subscriber_metadata(path: Path) -> list[str]:
    violations: list[str] = []
    value = _load_json(path, "subscriber-metadata.json", violations)
    if value is None:
        return violations
    required = {
        "started_at_ns", "ended_at_ns", "exit_reason", "git_sha", "host_tag",
        "sequence_end_exclusive", "ignored_sequence_count", "unexpected_sequence_count",
        "total_recorded", "total_messages", "parse_errors", "negative_latency_count",
        "latency_p50_ns", "latency_p95_ns", "latency_p99_ns",
        "histogram_lowest_ns", "histogram_highest_ns", "histogram_sig_digits", "sequence",
    }
    violations.extend(
        f"subscriber-metadata.json missing field: {field}"
        for field in sorted(required - value.keys())
    )
    if not required <= value.keys():
        return violations
    sequence = value["sequence"]
    sequence_fields = {"total_received", "total_gaps", "total_duplicates"}
    if not isinstance(sequence, dict) or not sequence_fields <= sequence.keys():
        return violations + ["subscriber-metadata.json sequence summary is invalid"]
    try:
        if int(value["ended_at_ns"]) <= int(value["started_at_ns"]):
            violations.append("subscriber-metadata.json measurement interval is invalid")
        if int(value["total_recorded"]) != int(sequence["total_received"]):
            violations.append("subscriber-metadata.json recorded count does not reconcile")
        if int(value["total_messages"]) != int(value["total_recorded"]):
            violations.append("subscriber-metadata.json message and HDR populations differ")
        if (
            int(value["histogram_lowest_ns"]) != 1_000
            or int(value["histogram_highest_ns"]) != 10_000_000_000
            or int(value["histogram_sig_digits"]) != 3
        ):
            violations.append("subscriber-metadata.json histogram precision differs from the frozen recorder")
        for field in ("unexpected_sequence_count", "parse_errors", "negative_latency_count"):
            if int(value[field]) != 0:
                violations.append(f"subscriber-metadata.json {field} must be zero")
    except (TypeError, ValueError):
        violations.append("subscriber-metadata.json counters and timestamps must be integers")
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
        "intended", "rejected", "enqueued", "received_events", "received_unique",
        "downstream_lost", "total_undelivered", "duplicates", "unexpected",
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
                if int(messages["enqueued"]) != int(messages["received_unique"]) + int(messages["downstream_lost"]):
                    violations.append("capacity-run.json enqueued counters do not reconcile")
                if int(messages["received_events"]) != int(messages["received_unique"]) + int(messages["duplicates"]):
                    violations.append("capacity-run.json received counters do not reconcile")
                if int(messages["total_undelivered"]) != int(messages["rejected"]) + int(messages["downstream_lost"]):
                    violations.append("capacity-run.json undelivered counters do not reconcile")
                if int(messages["unexpected"]) != 0:
                    violations.append("capacity-run.json contains unexpected sequences")
            except (TypeError, ValueError):
                violations.append("capacity-run.json message counters must be integers")
    if value.get("experiment") != expected_experiment:
        violations.append(f"capacity-run.json experiment must be {expected_experiment}")
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
        or histogram["highest_ns"] != 10_000_000_000
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
        "received_events": subscriber.get("total_recorded"),
        "duplicates": subscriber.get("sequence", {}).get("total_duplicates"),
        "unexpected": subscriber.get("unexpected_sequence_count"),
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
        received_events = int(subscriber["total_recorded"])
        duplicates = int(subscriber["sequence"]["total_duplicates"])
        received_unique = received_events - duplicates
        if intended != rejected + enqueued or received_unique > enqueued:
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
        "compile_ns", "instantiate_ns", "signal_ns", "ack_ns", "convergence_ns"
    } or any(type(duration) is not int or duration < 0 for duration in internal_phases.values()):
        violations.append("burst-timeline.json internal swap phases are invalid")
    return violations


def check_swap4_reconciliation(leaf: Path) -> list[str]:
    violations: list[str] = []
    timeline = _load_json(leaf / "burst-timeline.json", "burst-timeline.json", violations)
    throughput = _load_json(
        leaf / "throughput-buckets.json", "throughput-buckets.json", violations
    )
    sink_timeline = _load_json(leaf / "swap_timeline.json", "swap_timeline.json", violations)
    analysis = _load_json(leaf / "hotswap-analysis.json", "hotswap-analysis.json", violations)
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
    if None in (timeline, throughput, sink_timeline, analysis, source_timing, source_summary):
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
        elif timeline["swap_ns"] != requests[0]["request_started_ns"]:
            violations.append("E-Swap-4 actual swap timestamp differs from its request")
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


def _sequence_violations(path: Path) -> list[str]:
    try:
        with path.open(newline="") as stream:
            reader = csv.DictReader(stream)
            rows = list(reader)
            fields = set(reader.fieldnames or [])

        summary_fields = {
            "total_expected",
            "total_received",
            "gap_msgs",
            "duplicates_count",
        }
        event_fields = {"event_type", "seq_start", "seq_end", "count"}
        if summary_fields <= fields:
            if len(rows) != 1:
                return ["sequence.csv must contain one summary row"]
            expected = int(rows[0]["total_expected"])
            received = int(rows[0]["total_received"])
            gaps = int(rows[0]["gap_msgs"])
            duplicates = int(rows[0]["duplicates_count"])
        elif event_fields <= fields:
            metadata = json.loads((path.parent / "subscriber-metadata.json").read_text())
            sequence = metadata["sequence"]
            expected = int(metadata["total_messages"])
            received = int(sequence["total_received"])
            gaps = int(sequence["total_gaps"])
            duplicates = int(sequence["total_duplicates"])
            event_gaps = sum(int(row["count"]) for row in rows if row["event_type"] == "gap")
            event_duplicates = sum(
                int(row["count"]) for row in rows if row["event_type"] == "duplicate"
            )
            if event_gaps != gaps or event_duplicates != duplicates:
                return ["sequence.csv events differ from subscriber metadata"]
            if (
                metadata.get("exit_reason") != "total-messages"
                or int(metadata["total_recorded"]) != received
                or int(metadata.get("parse_errors", 0)) != 0
                or int(metadata.get("negative_latency_count", 0)) != 0
            ):
                return ["subscriber sequence metadata is inconsistent"]
        else:
            return ["sequence.csv has an unknown schema"]
    except (KeyError, OSError, TypeError, ValueError) as error:
        return [f"sequence.csv is invalid: {error}"]
    if expected != received or gaps != 0 or duplicates != 0:
        return [
            "sequence.csv is not lossless: "
            f"expected={expected}, received={received}, gaps={gaps}, duplicates={duplicates}"
        ]
    return []


def _focused_leaf_identity(leaf: Path, experiment: str) -> tuple[str, int] | None:
    parts = leaf.parts
    try:
        experiment_index = parts.index(experiment)
        batch_index = next(
            index
            for index in range(experiment_index + 1, len(parts))
            if parts[index].startswith("rpi5-")
        )
    except (ValueError, StopIteration):
        return None
    match = re.match(r"run-(\d+)(?:-attempt-\d+)?$", leaf.name)
    if match is None:
        return None
    condition = "/".join(parts[batch_index + 1 : -1])
    return condition, int(match.group(1))


def check_focused_matrix(matrix: dict) -> list[str]:
    focused = matrix.get("focused_pilot")
    if not isinstance(focused, dict):
        return ["canonical matrix lacks focused_pilot"]
    if focused.get("thesis_evidence") is not False:
        return ["focused_pilot must set thesis_evidence=false"]
    decisions = focused.get("decisions", {})
    concurrency = decisions.get("ekuiper_operator_concurrency", {})
    memory = decisions.get("memory_retention", {})
    violations = []
    if concurrency.get("value") != 1 or concurrency.get("comparison") != "default-system":
        violations.append("focused_pilot eKuiper concurrency decision is not frozen at default-system value 1")
    if memory.get("status") != "fixed":
        violations.append("focused_pilot memory-retention status is not fixed")
    if memory.get("max_observed_slope_bytes_per_message") != 0.0:
        violations.append("focused_pilot memory-retention slope is not 0 bytes/message")
    if memory.get("canonical_measurement_secs_adequate") is not True:
        violations.append("focused_pilot memory-retention decision does not approve the measurement window")
    return violations


def check_focused_leaf(
    leaf: Path,
    experiment: str,
    metadata: dict,
    canonical_matrix: dict,
    matrix_sha256: str,
) -> list[str]:
    violations: list[str] = []
    focused = canonical_matrix["focused_pilot"]
    definition = focused.get("experiments", {}).get(experiment)
    identity = _focused_leaf_identity(leaf, experiment)
    if not isinstance(definition, dict) or identity is None:
        return [f"leaf is not selected by focused_pilot: {leaf}"]
    condition, run_index = identity
    if run_index not in definition.get("condition_runs", {}).get(condition, []):
        violations.append(
            f"focused_pilot does not select {experiment}/{condition}/run-{run_index:02d}"
        )
    if metadata.get("thesis_evidence") is not False:
        violations.append("focused pilot metadata must set thesis_evidence=false")
    provenance = metadata.get("focused_pilot", {})
    decisions = focused["decisions"]
    expected_provenance = {
        "id": focused["id"],
        "matrix_sha256": matrix_sha256,
        "memory_retention_fix_commit": decisions["memory_retention"]["fix_commit"],
        "ekuiper_operator_concurrency": decisions["ekuiper_operator_concurrency"]["value"],
    }
    if provenance != expected_provenance:
        violations.append("focused pilot metadata provenance differs from the frozen matrix")

    if experiment == "e-perf-10":
        sweep = _load_json(leaf / "rate-sweep.json", "rate-sweep.json", violations)
        if sweep is not None:
            violations.extend(check_rate_sweep_result(leaf / "rate-sweep.json"))
            if sweep.get("system") != metadata.get("system"):
                violations.append("rate-sweep system differs from metadata")
        subscriber = _load_json(
            leaf / "subscriber-metadata.json", "subscriber-metadata.json", violations
        )
        if subscriber is not None and sweep is not None:
            offered = sweep.get("messages", {}).get("offered")
            if subscriber.get("sequence_end_exclusive") != offered:
                violations.append(
                    "E-Perf-10 subscriber sequence boundary differs from offered messages"
                )
            ignored = subscriber.get("ignored_sequence_count")
            if not isinstance(ignored, int) or ignored < 0:
                violations.append(
                    "E-Perf-10 subscriber ignored sequence count is missing or invalid"
                )
        if metadata.get("system") == "ekuiper":
            audit = _load_json(leaf / "ekuiper-audit.json", "ekuiper-audit.json", violations)
            expected = focused["decisions"]["ekuiper_operator_concurrency"]["value"]
            if audit is not None and audit.get("rule", {}).get("options", {}).get("concurrency") != expected:
                violations.append("eKuiper audit does not use frozen operator concurrency 1")

    if experiment == "e-backpressure":
        result = _load_json(leaf / "backpressure.json", "backpressure.json", violations)
        if result is not None:
            if result.get("classification") != "saturated-and-drained":
                violations.append("backpressure classification must be saturated-and-drained")
            peak_occupancy = result.get("peak_occupancy")
            occupancy_threshold = result.get("occupancy_threshold")
            if (
                result.get("threshold_crossed") is not True
                or result.get("recovered") is not True
                or not isinstance(peak_occupancy, (int, float))
                or not isinstance(occupancy_threshold, (int, float))
                or peak_occupancy < occupancy_threshold
            ):
                violations.append("backpressure evidence did not cross and recover below thresholds")
            if set(result.get("rates_msg_s", {})) != {"offered", "accepted", "processed", "drained"}:
                violations.append("backpressure rates must separate offered, accepted, processed, and drained")
            if result.get("rates_msg_s", {}).get("drained") is None:
                violations.append("backpressure drain rate is missing")
            if result.get("sequence", {}).get("lossless") is not True:
                violations.append("backpressure slow policy is not lossless")
            if result.get("memory", {}).get("within_limit") is not True:
                violations.append("backpressure RSS exceeds the frozen limit")

    if experiment == "e-iso-4":
        result = _load_json(leaf / "containment.json", "containment.json", violations)
        if result is not None:
            traps = int(result.get("traps_total", 0))
            recoveries = sum(int(node.get("recovery_count", 0)) for node in result.get("nodes", []))
            if result.get("contained") is not True or traps <= 0:
                violations.append("epoch containment did not record a contained trap")
            if recoveries != traps:
                violations.append(f"epoch recovery count differs from traps: {recoveries} != {traps}")
        try:
            if "cannot enter component instance" in (leaf / "stdout.log").read_text(errors="replace"):
                violations.append("epoch recovery reused an interrupted component instance")
        except OSError:
            pass

    if experiment == "e-iso-7":
        result = _load_json(leaf / "branch-isolation.json", "branch-isolation.json", violations)
        if result is not None:
            if result.get("condition") != condition:
                violations.append("branch-isolation condition differs from the leaf")
            if result.get("measurement_boundary", {}).get("kind") != "branch_sink_post_warmup":
                violations.append("branch-isolation measurement boundary is not branch-local")
            branches = result.get("branches", {})
            branch_a = branches.get("branch_a", {})
            branch_b = branches.get("branch_b", {})
            if (
                branch_a.get("artifact_dir") != "branch-a"
                or branch_b.get("artifact_dir") != "branch-b"
            ):
                violations.append("branch-isolation evidence does not use independent branch sinks")
            if (
                branch_a.get("source_node") != "source_a"
                or branch_b.get("source_node") != "source_b"
                or branch_a.get("source_node") == branch_b.get("source_node")
            ):
                violations.append("branch-isolation evidence does not use independent source populations")
            target = branch_a.get("target_messages")
            offered = branch_a.get("offered_messages")
            received = branch_a.get("received_messages")
            if (
                branch_a.get("sequence_scope") != "post_warmup"
                or not isinstance(target, int)
                or not isinstance(offered, int)
                or target < offered
                or branch_a.get("target_shortfall_messages") != target - offered
                or branch_a.get("target_shortfall_messages") != 0
                or offered != received
                or branch_a.get("lost_messages") != 0
                or branch_a.get("gap_messages") != 0
                or branch_a.get("duplicates") != 0
            ):
                violations.append("branch A is not lossless and independently attributed")
            throughput = branch_a.get("throughput", {})
            latency = branch_a.get("latency_ns", {})
            window = branch_a.get("measurement_window", {})
            duration_ns = (
                window.get("finished_ns", 0) - window.get("started_ns", 0)
                if isinstance(window, dict)
                else 0
            )
            expected_duration_ns = int(definition["measurement_secs"]) * 1_000_000_000
            if (
                throughput.get("total_messages") != received
                or latency.get("sample_count") != received
                or abs(duration_ns - expected_duration_ns) > 2_000_000_000
            ):
                violations.append("branch A counts do not share the measurement boundary")

    if experiment == "e-perf-9":
        result = _load_json(leaf / "startup.json", "startup.json", violations)
        if result is not None:
            phases = result.get("phases_ns", {})
            required_phases = {
                "process_config", "component_load_compile", "instantiation",
                "pipeline_setup", "first_process",
            }
            if set(phases) != required_phases or any(type(value) is not int or value < 0 for value in phases.values()):
                violations.append("startup phases must be complete non-negative nanosecond values")
            if result.get("processed_messages") != 1:
                violations.append("startup evidence must contain exactly one processed message")
            cache = result.get("compiled_component_cache", {})
            if cache.get("mode") == "disabled" and (
                cache.get("hit") is not False
                or cache.get("artifact") is not None
                or cache.get("identity") is not None
            ):
                violations.append("disabled compiled cache reports unsupported hit evidence")

    if experiment in {"e-swap-1", "e-swap-2", "e-swap-4", "e-swap-6"}:
        result = _load_json(leaf / "hotswap-analysis.json", "hotswap-analysis.json", violations)
        expected_events = definition.get("events_per_run")
        if result is not None:
            events = result.get("events")
            if result.get("duration_unit") != "ns" or not isinstance(events, list):
                violations.append("hot-swap analysis must contain nanosecond event records")
            elif result.get("sample_count") != expected_events or len(events) != expected_events:
                violations.append(f"hot-swap analysis must contain {expected_events} events")
            else:
                required = {
                    "compile_ns", "instantiate_ns", "signal_ns", "ack_ns",
                    "convergence_ns", "http_total_ns", "sink_observed_output_gap_ns",
                }
                if any(
                    any(type(event.get(field)) is not int or event[field] < 0 for field in required)
                    for event in events
                ):
                    violations.append("hot-swap event phases must be non-negative nanoseconds")

    if experiment.startswith("e-swap-"):
        violations.extend(_sequence_violations(leaf / "sequence.csv"))
    if experiment == "e-swap-5":
        result = _load_json(leaf / "rollback.json", "rollback.json", violations)
        if result is not None:
            expected_events = definition.get("events_per_run")
            if (
                result.get("attempts") != expected_events
                or result.get("rolled_back") != expected_events
                or result.get("all_rolled_back") is not True
            ):
                violations.append(f"rollback evidence must show {expected_events} successful rollbacks")
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
            or not 60 <= int(interval["row_count"]) <= 62
            or int(interval["measurement_end_ns"])
            - int(interval["measurement_start_ns"])
            != 60_000_000_000
            or hashlib.sha256(interval_path.read_bytes()).hexdigest()
            != interval.get("sha256")
        ):
            violations.append("eKuiper runtime summary interval alignment is invalid")
    except (KeyError, OSError, TypeError, ValueError):
        violations.append("eKuiper runtime summary interval alignment is invalid")
    gc_runtime = runtime.get("gc_runtime_metrics")
    if gc_runtime != {
        "status": "unavailable",
        "reason": "ekuiper-2.1.0-has-no-validated-gc-event-interface",
    }:
        violations.append("eKuiper GC/runtime limitation is missing or overstated")
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
        if (
            sequence != expected_sequence
            or expected_sequence["expected"] != expected_sequence["received"]
            or expected_sequence["gaps"] != 0
            or expected_sequence["duplicates"] != 0
        ):
            raise ValueError
    except (KeyError, OSError, TypeError, ValueError):
        violations.append(f"{artifact_name} sequence evidence is not lossless or reconciled")
    required = (
        {"compile_ns", "instantiate_ns", "signal_ns", "ack_ns", "convergence_ns", "http_total_ns", "sink_observed_output_gap_ns"}
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
                fields |= {"ack_ns", "convergence_ns"}
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


def check_leaf(
    leaf: Path,
    experiment: str,
    canonical: bool = False,
    canonical_matrix: dict | None = None,
    focused: bool = False,
    matrix_sha256: str = "",
) -> tuple[list[str], list[str]]:
    """Return (violations, warnings). Empty lists = fully conformant."""
    files = {f.name for f in leaf.iterdir() if f.is_file()}
    violations: list[str] = []
    warnings: list[str] = []

    for core in CORE_FILES:
        if core in files:
            continue
        # Legacy scripts predating F2 don't emit metadata.json — flag as
        # a known gap, not a fresh regression. The scripts themselves are
        # tracked by P-Followup-2 tail (not shipped this plan).
        if core == "metadata.json" and experiment in LEGACY_METADATA_MISSING_ALLOWED:
            continue
        violations.append(f"missing core artefact: {core}")

    # T8: warn (don't fail) when metadata.json is present but lacks the
    # provenance keys that `write_metadata.py` merges from
    # `runtime-provenance.json`. Legacy shakedown scripts write bespoke
    # per-experiment metadata schemas by design and are documented as
    # such in their headers; the WARN surfaces the gap without breaking
    # existing baseline dirs.
    meta_path = leaf / "metadata.json"
    metadata: dict = {}
    if meta_path.is_file():
        try:
            with meta_path.open() as fh:
                metadata = json.load(fh)
            missing = [k for k in MERGED_PROVENANCE_KEYS if k not in metadata]
            if missing and metadata.get("system") not in {"ekuiper", "mqtt-loopback", "static"}:
                message = (
                    f"metadata.json lacks merged provenance keys: {sorted(missing)} "
                    f"(legacy shakedown script; canonical runs source "
                    f"eval/scripts/lib/write_metadata.py)"
                )
                if canonical:
                    violations.append(message)
                else:
                    warnings.append(message)
            if leaf.name.startswith("rpi5-") or canonical:
                expected = {
                    "host_tag": "rpi5",
                    "arch": "aarch64",
                    "isolated_cpus": "1-3",
                    "throttled": "0x0",
                }
                for key, value in expected.items():
                    if metadata.get(key) != value:
                        violations.append(
                            f"Pi 5 metadata {key}={metadata.get(key)!r}, expected {value!r}"
                        )
                if "Raspberry Pi 5" not in str(metadata.get("hardware_model", "")):
                    violations.append("Pi 5 metadata lacks Raspberry Pi 5 hardware model")
                if metadata.get("cpu_governors") != ["performance"]:
                    violations.append("Pi 5 metadata CPU governor is not performance")
                if not re.fullmatch(r"[0-9a-f]{40}", str(metadata.get("git_sha", ""))):
                    violations.append("Pi 5 metadata lacks a source commit SHA")
                if not isinstance(metadata.get("git_dirty"), bool):
                    violations.append("Pi 5 metadata git_dirty is not boolean")
                exit_codes = metadata.get("exit_codes", {})
                if metadata.get("system") == "ekuiper":
                    if exit_codes.get("ekuiper") != 0:
                        violations.append("Pi 5 metadata records a non-zero eKuiper exit")
                elif metadata.get("system") == "mqtt-loopback":
                    if exit_codes.get("publisher") != 0 or exit_codes.get("subscriber") != 0:
                        violations.append("Pi 5 metadata records a non-zero loopback loadgen exit")
                elif metadata.get("system") == "static":
                    if exit_codes.get("collector") != 0:
                        violations.append("Pi 5 metadata records a non-zero static collector exit")
                elif exit_codes.get("wafer_runtime") != 0:
                    violations.append("Pi 5 metadata records a non-zero runtime exit")
                if experiment == "e-perf-10":
                    expected_evidence = False if focused else True
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
                    if not focused and metadata.get("system") == "wafer" and canonical_matrix is not None:
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
                    tags = metadata.get("git_tags")
                    if not isinstance(tags, list) or not tags:
                        violations.append("canonical result lacks tagged source provenance")
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
            try:
                window = json.loads(window_path.read_text())
                if int(window["finished_ns"]) <= int(window["started_ns"]):
                    violations.append("canonical measurement window is empty or reversed")
            except (KeyError, OSError, TypeError, ValueError):
                violations.append("canonical measurement window is invalid")

    if canonical and canonical_matrix is not None:
        experiment_contract = (
            canonical_matrix.get("focused_pilot", {}).get("experiments", {}).get(experiment)
            if focused
            else canonical_matrix.get("experiments", {}).get(experiment)
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
            if interval_contract and timed_output and not focused:
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
    if experiment in CANDIDATE_SWAP_EXPERIMENTS:
        violations.extend(check_candidate_swap_evidence(leaf, metadata, experiment))
    if experiment == EKUIPER_PROFILE_EXPERIMENT:
        violations.extend(check_ekuiper_profile_artifacts(leaf, metadata))
    if experiment == "e-perf-10" and "rate-sweep.json" in files:
        violations.extend(check_rate_sweep_result(leaf / "rate-sweep.json"))
    if experiment == "e-perf-10" and not focused:
        for historical in ("rate-sweep.json", "published.csv", "received.csv"):
            if historical in files:
                violations.append(f"final E-Perf-10 must not contain historical trace artifact: {historical}")
    if experiment == "e-perf-10" and "capacity-run.json" in files:
        violations.extend(check_publisher_summary(leaf / "publisher-summary.json"))
        violations.extend(check_subscriber_metadata(leaf / "subscriber-metadata.json"))
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
    if experiment == "e-swap-3" and not focused:
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
        violations.extend(check_disruption_analysis(leaf / "disruption-analysis.json"))
        violations.extend(check_swap3_reconciliation(leaf, metadata))
    if experiment == "e-swap-4" and not focused:
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

    if focused and canonical_matrix is not None:
        violations.extend(
            check_focused_leaf(leaf, experiment, metadata, canonical_matrix, matrix_sha256)
        )

    matrix = OPTIONAL_MATRIX.get(experiment)
    if matrix is None:
        return violations, warnings  # Unknown experiment id — core-only check.

    for f in matrix["must_have"]:
        if f not in files:
            violations.append(f"missing required optional artefact for {experiment}: {f}")
    for f in matrix.get("must_not_have", set()):
        if f in files:
            violations.append(f"unexpected artefact for {experiment}: {f}")
    return violations, warnings


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__.split("\n\n", 1)[0])
    parser.add_argument(
        "--canonical",
        action="store_true",
        help="enforce Pi 5 provenance and experiment-specific canonical outputs",
    )
    parser.add_argument(
        "--focused",
        action="store_true",
        help="enforce the frozen focused-pilot selection and semantic invariants",
    )
    parser.add_argument("--matrix", type=Path, default=CANONICAL_MATRIX)
    parser.add_argument("dirs", nargs="+", type=Path)
    args = parser.parse_args()

    if args.focused and not args.canonical:
        parser.error("--focused requires --canonical")

    canonical_matrix: dict | None = None
    matrix_sha256 = ""
    if args.canonical:
        try:
            matrix_bytes = args.matrix.read_bytes()
            canonical_matrix = json.loads(matrix_bytes)
            matrix_sha256 = hashlib.sha256(matrix_bytes).hexdigest()
            if args.focused:
                freeze_path = args.matrix.with_name("focused-pilot-freeze.json")
                if not freeze_path.is_file():
                    freeze_path = CANONICAL_MATRIX.with_name("focused-pilot-freeze.json")
                freeze = json.loads(freeze_path.read_text())
                matrix_sha256 = freeze["canonical_matrix_sha256"]
        except (OSError, ValueError) as exc:
            print(f"error: cannot load canonical matrix: {exc}", file=sys.stderr)
            return 2

    all_violations: list[tuple[Path, str]] = []
    all_warnings: list[tuple[Path, str]] = []
    canonical_shas: set[str] = set()
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
            violations, warnings = check_leaf(
                leaf,
                experiment,
                canonical=args.canonical,
                canonical_matrix=canonical_matrix,
                focused=args.focused,
                matrix_sha256=matrix_sha256,
            )
            if args.canonical:
                try:
                    metadata = json.loads((leaf / "metadata.json").read_text())
                    sha = metadata.get("git_sha")
                    if isinstance(sha, str):
                        canonical_shas.add(sha)
                except (OSError, ValueError):
                    pass
            for v in violations:
                all_violations.append((leaf, v))
            for w in warnings:
                all_warnings.append((leaf, w))

    if args.canonical and len(canonical_shas) > 1:
        all_violations.append(
            (Path("<batch>"), f"canonical inputs mix source SHAs: {sorted(canonical_shas)}")
        )
    if args.focused and canonical_matrix is not None:
        all_violations.extend(
            (Path("<focused-pilot>"), violation)
            for violation in check_focused_matrix(canonical_matrix)
        )

    for leaf, w in all_warnings:
        print(f"WARN       {leaf}: {w}")

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
