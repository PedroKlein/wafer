"""Presentation labels and admitted-leaf readers for the analysis notebooks."""

from __future__ import annotations

import csv
import json
import re
from pathlib import Path

import pandas as pd

from .attempts import INCOMPLETE_RUN_REASONS, batch_units

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


def admitted_runs(batch: Path, artifact: str) -> list[dict]:
    """One record per admitted unit: its artifact, run identity and outcome reasons.

    The record of a unit whose system under test stopped the run early holds only
    ``condition``, ``run_index`` and ``sut_outcome_reasons`` when the artifact is absent.
    """
    records = []
    for unit in batch_units(batch, None):
        attempt = unit.admitted
        if attempt is None:
            continue
        path = attempt.path / artifact
        if path.is_file():
            value = json.loads(path.read_text())
        elif INCOMPLETE_RUN_REASONS & set(attempt.reasons):
            value = {}
        else:
            raise ValueError(f"admitted attempt lacks {artifact}: {attempt.path}")
        records.append(
            {
                "condition": unit.condition,
                "run_index": unit.run_index,
                **value,
                "sut_outcome_reasons": list(attempt.reasons),
            }
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
        if not throughput_path.is_file() or not subscriber_path.is_file():
            raise ValueError(
                f"target-load leaf lacks throughput or subscriber evidence: {path.parent}"
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
