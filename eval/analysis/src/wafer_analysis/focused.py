"""Focused-pilot presentation labels and artifact inventory."""

from __future__ import annotations

import csv
import json
from pathlib import Path

import pandas as pd

FOLLOWUP_ARTIFACTS = (
    ("eKuiper latency tail", "09-saturation.ipynb", "rate-sweep.json"),
    ("target load versus saturation", "09-saturation.ipynb", "rate-sweep-summary.json"),
    (
        "branch-A throughput and latency",
        "06-fault-injection.ipynb",
        "branch-isolation.json",
    ),
    ("epoch recovery", "06-fault-injection.ipynb", "containment.json"),
    ("startup cache state", "10-aot-startup.ipynb", "startup.json"),
    ("bounded queue pressure", "09-backpressure.ipynb", "backpressure.json"),
    (
        "internal and sink-observed hot-swap timing",
        "05-hotswap-timeline.ipynb",
        "hotswap-analysis.json",
    ),
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


def passed_artifacts(batch: Path, artifact: str) -> list[tuple[Path, dict]]:
    results = []
    for path in sorted(batch.rglob(artifact)):
        status_path = path.parent / "canonical-status.json"
        if not status_path.is_file():
            continue
        status = json.loads(status_path.read_text())
        if status.get("status") != "passed":
            continue
        results.append((path, json.loads(path.read_text())))
    return results


def percentile_rows(batch: Path) -> pd.DataFrame:
    rows = []
    for path, values in passed_artifacts(batch, "percentiles.json"):
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
    for path, values in passed_artifacts(batch, "percentiles.json"):
        throughput_path = path.parent / "throughput.csv"
        sequence_path = path.parent / "sequence.csv"
        if not throughput_path.is_file() or not sequence_path.is_file():
            raise ValueError(
                f"target-load leaf lacks throughput or sequence evidence: {path.parent}"
            )
        with throughput_path.open(newline="") as stream:
            throughput_rows = list(csv.DictReader(stream))
        if len(throughput_rows) != 1:
            raise ValueError(
                f"target-load throughput must contain one run summary: {throughput_path}"
            )
        throughput = throughput_rows[0]
        with sequence_path.open(newline="") as stream:
            sequence_rows = list(csv.DictReader(stream))
        gaps = sum(
            int(row["count"]) for row in sequence_rows if row["event_type"] == "gap"
        )
        duplicates = sum(
            int(row["count"])
            for row in sequence_rows
            if row["event_type"] == "duplicate"
        )
        received_events = int(throughput["messages_received"])
        received_unique = received_events - duplicates
        duration_ns = int(throughput["duration_ns"])
        if (
            received_unique < 0
            or received_unique > intended
            or duration_ns <= 0
            or int(values["total_count"]) != received_events
        ):
            raise ValueError(f"target-load counters do not reconcile: {path.parent}")
        if intended - received_unique != gaps:
            raise ValueError(f"target-load gap total does not reconcile: {path.parent}")
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
            }
        )
    return pd.DataFrame(rows)


def artifact_inventory(available: set[str] | None = None) -> pd.DataFrame:
    available = available or set()
    rows = []
    for question, notebook, artifact in FOLLOWUP_ARTIFACTS:
        rows.append(
            {
                "question": question,
                "notebook": notebook,
                "artifact": artifact,
                "status": "READY" if artifact in available else "PENDING",
                "thesis_evidence": False,
            }
        )
    return pd.DataFrame(rows)
