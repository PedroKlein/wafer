"""Presentation labels and admitted-leaf readers for the analysis notebooks."""

from __future__ import annotations

import base64
import csv
import json
import math
import re
import struct
from bisect import bisect_left
from itertools import accumulate
from pathlib import Path

import pandas as pd

from .attempts import INCOMPLETE_RUN_REASONS, PASSED, batch_units

_ANSI_ESCAPE = re.compile(r"\x1b\[[0-9;]*m")
_PUBLISHER_DONE = re.compile(r"Load generation complete total=\d+ errors=\d+ acked=(\d+)")
_CAPACITY_COUNTERS = ("intended", "rejected", "acked", "received_unique")
_EKUIPER_SUT_COUNTERS = (
    "source_wafer_telemetry_0_records_in_total",
    "sink_mqtt_0_0_records_out_total",
)

def evidence_label(sample_count: int, units: str, thesis_evidence: bool) -> str:
    evidence = "thesis" if thesis_evidence else "diagnostic"
    uncertainty = (
        "inferential" if thesis_evidence and sample_count >= 30 else "descriptive only"
    )
    return f"N={sample_count}; units={units}; evidence={evidence}; uncertainty={uncertainty}"


def pending_record(question: str, reason: str, units: str) -> dict:
    return {
        "question": question,
        "status": "PENDING",
        "value": None,
        "units": units,
        "reason": reason,
        "thesis_evidence": False,
    }


def admitted_artifacts(batch: Path, artifact: str) -> list[tuple[Path, dict]]:
    """The artifact of each unit's admitted attempt, a clean pass or a system outcome.

    A unit whose system under test stopped the run early may lack the artifact; use
    ``admitted_runs`` where such a unit must count against a criterion.
    """
    results = []
    for unit in batch_units(batch, None):
        if unit.admitted is None:
            continue
        path = unit.admitted.path / artifact
        if path.is_file():
            results.append((path, json.loads(path.read_text())))
    return results


_V2_HISTOGRAM = struct.Struct(">IIiIqqd")
_V2_COOKIE = 0x1C849313
_HDR_PERCENTILES = {"p50_ns": 0.5, "p95_ns": 0.95, "p99_ns": 0.99, "p999_ns": 0.999}


def _zigzag_varints(payload: bytes):
    offset = 0
    while offset < len(payload):
        value = shift = 0
        while True:
            byte = payload[offset]
            offset += 1
            if shift == 56:
                value |= byte << 56
                break
            value |= (byte & 0x7F) << shift
            if byte < 0x80:
                break
            shift += 7
        yield (value >> 1) ^ -(value & 1)


def _hdr_summary(path: Path) -> dict:
    """The ``total_count`` and ``p50_ns`` to ``p999_ns`` of a BenchSink interval log.

    For a log with at least one sample the values equal what ``wafer-loadgen hdr-summary``
    writes for the same file; a log without samples raises instead of reporting zeros.
    """
    counts: dict[int, int] = {}
    layout = None
    for line in path.read_text().splitlines():
        if not line or line.startswith("#"):
            continue
        histogram = base64.b64decode(line.rsplit(",", 1)[1])
        cookie, length, _, digits, lowest, _, _ = _V2_HISTOGRAM.unpack_from(histogram)
        if cookie != _V2_COOKIE or layout not in (None, (digits, lowest)):
            raise ValueError(f"unsupported histogram encoding in {path}")
        layout = (digits, lowest)
        index = 0
        for value in _zigzag_varints(histogram[_V2_HISTOGRAM.size : _V2_HISTOGRAM.size + length]):
            if value < 0:
                index -= value
            else:
                counts[index] = counts.get(index, 0) + value
                index += 1
    total = sum(counts.values())
    if total == 0:
        raise ValueError(f"{path} holds no samples")
    digits, lowest = layout
    half_magnitude = math.ceil(math.log2(2 * 10**digits)) - 1
    unit_magnitude = math.floor(math.log2(lowest))
    indices = sorted(counts)
    cumulative = list(accumulate(counts[index] for index in indices))
    summary = {"total_count": total}
    for name, quantile in _HDR_PERCENTILES.items():
        index = indices[bisect_left(cumulative, max(1, math.ceil(quantile * total)))]
        bucket = max(0, (index >> half_magnitude) - 1)
        # HdrHistogram reports a percentile as the highest value its counts index stands for.
        summary[name] = ((index - (bucket << half_magnitude) + 1) << (bucket + unit_magnitude)) - 1
    return summary


def admitted_runs(batch: Path, artifact: str) -> list[dict]:
    """One record per admitted unit: its artifact, run identity and outcome reasons.

    A BenchSink ``.hdr`` artifact with at least one sample contributes the ``total_count``
    and ``p50_ns`` to ``p999_ns`` that ``wafer-loadgen hdr-summary`` writes for it. The
    record of a unit whose system under test stopped the run early holds only
    ``condition``, ``run_index`` and ``sut_outcome_reasons`` when the artifact is absent.
    """
    return [record for _, record in _admitted_leaf_runs(batch, artifact)]


def branch_isolation_runs(batch: Path) -> list[dict]:
    """``admitted_runs`` of ``branch-isolation.json``, each with branch A's arrival span.

    ``branch_a_arrival_span_ns`` runs from the first post-warmup arrival to the end of the
    last row of ``branch-a/interval-latency.json``, which the sink closes when its input
    ends, before it exports its files. It is None when that file is absent.
    """
    records = []
    for leaf, record in _admitted_leaf_runs(batch, "branch-isolation.json"):
        intervals = leaf / "branch-a" / "interval-latency.json"
        rows = json.loads(intervals.read_text())["rows"] if intervals.is_file() else []
        span = rows[-1]["interval_end_ns"] - rows[0]["interval_start_ns"] if rows else None
        records.append({**record, "branch_a_arrival_span_ns": span})
    return records


def _admitted_leaf_runs(batch: Path, artifact: str) -> list[tuple[Path, dict]]:
    records = []
    for unit in batch_units(batch, None):
        attempt = unit.admitted
        if attempt is None:
            continue
        path = attempt.path / artifact
        if path.is_file():
            value = _hdr_summary(path) if path.suffix == ".hdr" else json.loads(path.read_text())
        elif INCOMPLETE_RUN_REASONS & set(attempt.reasons):
            value = {}
        else:
            raise ValueError(f"admitted attempt lacks {artifact}: {attempt.path}")
        records.append(
            (
                attempt.path,
                {
                    "condition": unit.condition,
                    "run_index": unit.run_index,
                    **value,
                    "sut_outcome_reasons": list(attempt.reasons),
                },
            )
        )
    return records


def run_index(leaf: Path) -> int:
    """The run number of a ``run-N`` or ``run-N-attempt-M`` result directory."""
    match = re.fullmatch(r"run-(\d+)(?:-attempt-\d+)?", leaf.name)
    if match is None:
        raise ValueError(f"malformed run directory: {leaf}")
    return int(match.group(1))


def percentile_rows(batch: Path) -> pd.DataFrame:
    rows = []
    for path, values in admitted_artifacts(batch, "percentiles.json"):
        relative = path.relative_to(batch)
        rows.append(
            {
                "condition": relative.parts[0],
                "run": relative.parts[1]
                if len(relative.parts) > 2
                else path.parent.name,
                "N_messages": values.get("total_count"),
                "p50_ns": values.get("p50_ns"),
                "p95_ns": values.get("p95_ns"),
                "p99_ns": values.get("p99_ns"),
                "p999_ns": values.get("p999_ns"),
            }
        )
    return pd.DataFrame(rows)


def target_load_rows(
    batch: Path, *, rate_msg_s: int = 1_000, measurement_secs: int = 60
) -> pd.DataFrame:
    rows = []
    intended = rate_msg_s * measurement_secs
    for path, values in admitted_artifacts(batch, "percentiles.json"):
        throughput_path = path.parent / "throughput.csv"
        subscriber_path = path.parent / "subscriber-metadata.json"
        publisher_path = path.parent / "publisher-summary.json"
        if not all(evidence.is_file() for evidence in (throughput_path, subscriber_path, publisher_path)):
            raise ValueError(
                f"target-load leaf lacks throughput, subscriber or publisher evidence: {path.parent}"
            )
        with throughput_path.open(newline="") as stream:
            throughput_rows = list(csv.DictReader(stream))
        if len(throughput_rows) != 1:
            raise ValueError(
                f"target-load throughput must contain one run summary: {throughput_path}"
            )
        throughput = throughput_rows[0]
        subscriber = json.loads(subscriber_path.read_text())
        sequence = subscriber["sequence"]
        if (
            subscriber.get("sequence_end_exclusive") != intended
            or int(sequence["expected"]) != intended
        ):
            raise ValueError(
                f"target-load leaf does not declare its measured sequence range: {path.parent}"
            )
        gaps = int(sequence["total_gaps"])
        duplicates = int(sequence["total_duplicates"])
        received_events = int(throughput["messages_received"])
        received_unique = int(sequence["received_unique"])
        duration_ns = int(throughput["duration_ns"])
        if (
            received_unique < 0
            or received_unique > intended
            or received_events != received_unique + duplicates
            or duration_ns <= 0
            or int(values["total_count"]) != received_events
        ):
            raise ValueError(f"target-load counters do not reconcile: {path.parent}")
        if intended - received_unique != gaps:
            raise ValueError(f"target-load gap total does not reconcile: {path.parent}")
        source_lag = json.loads(publisher_path.read_text())["source_lag_ns"]
        relative = path.relative_to(batch)
        rows.append(
            {
                "condition": relative.parts[0],
                "run": relative.parts[1]
                if len(relative.parts) > 2
                else path.parent.name,
                "N_messages": values.get("total_count"),
                "p50_ns": values.get("p50_ns"),
                "p95_ns": values.get("p95_ns"),
                "p99_ns": values.get("p99_ns"),
                "p999_ns": values.get("p999_ns"),
                "intended_messages": intended,
                "received_unique": received_unique,
                "loss_fraction": (intended - received_unique) / intended,
                "achieved_rate_msg_s": received_unique / measurement_secs,
                "achieved_ratio": received_unique / intended,
                "duplicates": duplicates,
                "publisher_lag_p50_ns": source_lag["p50"],
                "publisher_lag_p99_ns": source_lag["p99"],
            }
        )
    return pd.DataFrame(rows)


def depth_run_records(batch: Path, *, canonical: bool) -> list[dict]:
    """One record per admitted MQTT depth run; a canonical run adds its declared-range loss."""
    if not canonical:
        return [
            {
                "condition": path.relative_to(batch).parts[0],
                "run_index": run_index(path.parent),
                "p50_ns": values["p50_ns"],
                "p95_ns": values["p95_ns"],
            }
            for path, values in admitted_artifacts(batch, "percentiles.json")
        ]
    return [
        {
            "condition": row["condition"],
            "run_index": run_index(Path(row["run"])),
            "p50_ns": row["p50_ns"],
            "p95_ns": row["p95_ns"],
            "received_unique": row["received_unique"],
            "loss_fraction": row["loss_fraction"],
        }
        for row in target_load_rows(batch).to_dict("records")
    ]


def capacity_loss_records(batch: Path) -> list[dict]:
    """One record per passed E-Perf-10 run: the messages counted at each hop of its delivery path.

    WAFER and native count the SUT's input and output in ``per_node_metrics.csv`` (what
    ``mqtt-in`` and ``mqtt-out`` emitted), eKuiper in its rule's source and sink counters between
    the two ``ekuiper-health.json`` snapshots. Both span warm-up and measurement, so the record
    keeps the warm-up publisher's acked count, which only its line in ``stdout.log`` holds. MQTT
    loopback has no SUT and gets ``None`` for these three.
    """
    records = []
    for unit in batch_units(batch, None):
        attempt = unit.admitted
        if attempt is None or attempt.outcome != PASSED:
            continue
        leaf = attempt.path
        if not (leaf / "capacity-run.json").is_file():
            continue
        run = json.loads((leaf / "capacity-run.json").read_text())
        messages = run["messages"]
        missing = [name for name in _CAPACITY_COUNTERS if name not in messages]
        if missing:
            raise ValueError(f"capacity-run.json lacks {', '.join(missing)}: {leaf}")
        record = {
            "system": run["system"],
            "offered_rate_msg_s": int(run["rate_msg_s"]),
            "run_index": int(run["run_index"]),
            **{name: int(messages[name]) for name in _CAPACITY_COUNTERS},
            "warmup_acked": None,
            "sut_input": None,
            "sut_output": None,
        }
        if run["system"] != "mqtt-loopback":
            record.update(_sut_counts(leaf, run["system"], record["acked"]))
        records.append(record)
    return records


def _sut_counts(leaf: Path, system: str, acked: int) -> dict:
    log = _ANSI_ESCAPE.sub("", (leaf / "stdout.log").read_text(errors="replace"))
    publishers = [int(value) for value in _PUBLISHER_DONE.findall(log)]
    if len(publishers) != 2 or publishers[1] != acked:
        raise ValueError(
            f"stdout.log does not record the warm-up publisher before the measured one: {leaf}"
        )
    if system == "ekuiper":
        health = json.loads((leaf / "ekuiper-health.json").read_text())
        before, after = (health[name]["rule_status"] for name in ("before", "after"))
        sut_input, sut_output = (
            int(after[name]) - int(before[name]) for name in _EKUIPER_SUT_COUNTERS
        )
    else:
        with (leaf / "per_node_metrics.csv").open(newline="") as stream:
            nodes = {row["node_id"]: row for row in csv.DictReader(stream)}
        sut_input, sut_output = (
            int(nodes[node]["messages_out"]) for node in ("mqtt-in", "mqtt-out")
        )
    return {"warmup_acked": publishers[0], "sut_input": sut_input, "sut_output": sut_output}
