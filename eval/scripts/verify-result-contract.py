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
import re
import sys
from pathlib import Path

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


def check_capacity_run_result(path: Path) -> list[str]:
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
    if value.get("experiment") != "e-perf-10":
        violations.append("capacity-run.json experiment must be e-perf-10")
    try:
        measurement_secs = FINAL_CAPACITY_MEASUREMENT_SECS
        repetitions = FINAL_CAPACITY_REPETITIONS
        if not 1 <= int(value.get("run_index")) <= repetitions:
            violations.append("capacity-run.json run_index is outside the frozen repetitions")
        if int(value.get("measurement_duration_ns")) != measurement_secs * 1_000_000_000:
            violations.append("capacity-run.json measurement duration differs from the frozen matrix")
        if int(messages.get("intended")) != int(value.get("rate_msg_s")) * measurement_secs:
            violations.append("capacity-run.json intended population differs from rate times duration")
    except (KeyError, TypeError, ValueError):
        violations.append("capacity-run.json frozen duration/population fields are invalid")
    if value.get("batch_class") != "final-capacity":
        violations.append("capacity-run.json batch_class must be final-capacity")
    if value.get("thesis_evidence") is not True:
        violations.append("capacity-run.json must set thesis_evidence=true")
    if value.get("traces") is not False:
        violations.append("capacity-run.json final capture must be trace-free")
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


def check_throughput_buckets(path: Path, expected_count: int | None = None) -> list[str]:
    violations: list[str] = []
    value = _load_json(path, "throughput-buckets.json", violations)
    if value is None:
        return violations
    if value.get("clock") != "monotonic" or value.get("bucket_width_ns") != 100_000_000:
        violations.append("throughput-buckets.json must use monotonic 100 ms buckets")
    buckets = value.get("buckets")
    if not isinstance(buckets, list) or not buckets:
        return violations + ["throughput-buckets.json buckets must be non-empty"]
    if expected_count is not None and len(buckets) != expected_count:
        violations.append(f"throughput-buckets.json must contain {expected_count} buckets")
    previous_end = None
    total = 0
    for bucket in buckets:
        required = {"start_offset_ns", "end_offset_ns", "received_unique", "received_events", "duplicates", "rate_msg_s"}
        if not isinstance(bucket, dict) or not required <= bucket.keys():
            violations.append("throughput-buckets.json bucket schema is invalid")
            continue
        if int(bucket["end_offset_ns"]) - int(bucket["start_offset_ns"]) != 100_000_000:
            violations.append("throughput-buckets.json bucket width is inconsistent")
        if previous_end is not None and int(bucket["start_offset_ns"]) != previous_end:
            violations.append("throughput-buckets.json buckets are not contiguous")
        previous_end = int(bucket["end_offset_ns"])
        total += int(bucket["received_events"])
    if value.get("received_events") != total:
        violations.append("throughput-buckets.json totals do not reconcile")
    return violations


def check_disruption_timeline(path: Path) -> list[str]:
    violations: list[str] = []
    value = _load_json(path, "disruption-timeline.json", violations)
    if value is None:
        return violations
    required = {"clock", "strategy", "event_ns", "action_start_ns", "action_end_ns"}
    if not required <= value.keys():
        return violations + ["disruption-timeline.json is missing event fields"]
    if value["clock"] != "monotonic" or not (
        int(value["action_start_ns"]) <= int(value["event_ns"]) <= int(value["action_end_ns"])
    ):
        violations.append("disruption-timeline.json event/action clocks are invalid")
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
        "swap_offset_ns": 60_000_000_000,
        "burst_end_offset_ns": 65_000_000_000,
        "successful_swaps": 1,
    }
    for field, expected_value in expected.items():
        if value.get(field) != expected_value:
            violations.append(f"burst-timeline.json {field} must be {expected_value}")
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


def expected_metering(matrix: dict, experiment: str, condition: str) -> dict:
    if experiment == "e-perf-7":
        values = matrix["experiments"][experiment]["metering_modes"][condition]
    else:
        values = matrix["experiments"][experiment].get("metering_exceptions", {}).get(
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
            if missing and metadata.get("system") not in {"ekuiper", "mqtt-loopback"}:
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
                elif exit_codes.get("wafer_runtime") != 0:
                    violations.append("Pi 5 metadata records a non-zero runtime exit")
                if experiment == "e-perf-10":
                    expected_evidence = False if focused else True
                    if metadata.get("thesis_evidence") is not expected_evidence:
                        violations.append(
                            f"E-Perf-10 metadata must set thesis_evidence={str(expected_evidence).lower()}"
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
                        metadata.get("system") not in {"ekuiper", "mqtt-loopback"}
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
        )
        if not isinstance(experiment_contract, dict):
            violations.append(f"experiment {experiment} is absent from canonical matrix")
        else:
            for required in experiment_contract.get("required_outputs", []):
                if required not in files:
                    violations.append(
                        f"missing required canonical artefact for {experiment}: {required}"
                    )

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
    if experiment == "e-swap-3" and not focused:
        violations.extend(check_throughput_buckets(leaf / "throughput-buckets.json", 200))
        violations.extend(check_disruption_timeline(leaf / "disruption-timeline.json"))
    if experiment == "e-swap-4" and not focused:
        violations.extend(check_throughput_buckets(leaf / "throughput-buckets.json"))
        violations.extend(check_burst_timeline(leaf / "burst-timeline.json"))
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
        experiment = experiment_of(root)
        if experiment is None:
            print(f"warn: cannot infer experiment id from {root}", file=sys.stderr)
            continue
        leaves = find_leaf_dirs(root)
        if not leaves:
            print(f"warn: no leaf run dirs (config.toml) under {root}", file=sys.stderr)
            continue
        for leaf in leaves:
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
