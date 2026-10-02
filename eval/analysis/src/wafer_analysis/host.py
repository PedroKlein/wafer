"""Host-side evidence: per-core CPU use, the attempts table and batch progress."""

from __future__ import annotations

import datetime as dt
import json
from collections.abc import Mapping
from pathlib import Path

import pandas as pd

from .attempts import (
    INFRASTRUCTURE,
    PASSED,
    SUT_OUTCOME,
    Attempt,
    batch_units,
    infrastructure_retries,
)
from .paths import _expected_units, batch_bracket_rates

_BUSY_FIELDS = ("user", "nice", "system", "irq", "softirq", "steal")
_TOTAL_FIELDS = (*_BUSY_FIELDS, "idle", "iowait")


def core_utilisation(rows: list[dict]) -> pd.DataFrame:
    """Busy and iowait percent per core between its first and last ``cpu-cores.csv`` sample."""
    by_cpu: dict[int, list[dict]] = {}
    for row in rows:
        by_cpu.setdefault(int(row["cpu"]), []).append(row)
    result = []
    for cpu, samples in sorted(by_cpu.items()):
        samples.sort(key=lambda sample: int(sample["timestamp_ns"]))
        first, last = samples[0], samples[-1]
        delta = {field: int(last[field]) - int(first[field]) for field in _TOTAL_FIELDS}
        total = sum(delta.values())
        if len(samples) < 2 or total <= 0 or min(delta.values()) < 0:
            raise ValueError(f"cpu {cpu} needs two increasing samples")
        result.append(
            {
                "cpu": cpu,
                "busy_percent": 100 * sum(delta[field] for field in _BUSY_FIELDS) / total,
                "iowait_percent": 100 * delta["iowait"] / total,
            }
        )
    return pd.DataFrame(result)


def cpu_list(text: str) -> set[int]:
    """Parse a Linux CPU list such as ``1-3,5``."""
    cpus: set[int] = set()
    for part in text.split(","):
        low, _, high = part.strip().partition("-")
        cpus.update(range(int(low), int(high or low) + 1))
    return cpus


def attempts_table(batches: dict[str, Path], matrix: Mapping) -> pd.DataFrame:
    """Units, attempts and attempt classes per experiment, system and condition.

    Every attempt directory counts, with or without a receipt. A scheduled unit without an
    admitted attempt is missing, whether its retries are spent or it never ran.
    """
    rows = []
    for experiment, batch in batches.items():
        retries = infrastructure_retries(matrix, experiment)
        scheduled = _expected_units(
            matrix["experiments"][experiment],
            batch_bracket_rates(batch, matrix) if experiment == "e-perf-10" else None,
        )
        units = {(unit.condition, unit.run_index): unit for unit in batch_units(batch, retries)}
        for condition in sorted({condition for condition, _ in scheduled | units.keys()}):
            due = [key for key in scheduled if key[0] == condition]
            present = [unit for (name, _), unit in units.items() if name == condition]
            attempts = [attempt for unit in present for attempt in unit.attempts]
            outcomes = [attempt.outcome for attempt in attempts]
            rows.append(
                {
                    "experiment": experiment,
                    "system": _system(attempts),
                    "condition": condition,
                    "units": len(due),
                    "attempts": len(attempts),
                    "passed": outcomes.count(PASSED),
                    "sut_outcome": outcomes.count(SUT_OUTCOME),
                    "infrastructure": outcomes.count(INFRASTRUCTURE),
                    "retries": sum(unit.retries for unit in present),
                    "missing": sum(
                        key not in units or units[key].admitted is None for key in due
                    ),
                }
            )
    return pd.DataFrame(rows)


def _system(attempts: list[Attempt]) -> str | None:
    for attempt in reversed(attempts):
        try:
            system = json.loads((attempt.path / "metadata.json").read_text()).get("system")
        except (OSError, ValueError, AttributeError):
            continue
        if isinstance(system, str):
            return system
    return None


def item_progress(progress: list[dict]) -> pd.DataFrame:
    """One row per finished batch item from ``progress.jsonl``, with wall time and temperature."""
    started: dict[str, dict] = {}
    rows = []
    for entry in progress:
        item = entry.get("item")
        if entry.get("event") == "item-started":
            started[item] = entry
        elif entry.get("event") == "item-finished" and item in started:
            begin = started.pop(item)
            start = _timestamp(begin["timestamp"])
            end = _timestamp(entry["timestamp"])
            rows.append(
                {
                    "item": item,
                    "experiment": item.split("/", 1)[0],
                    "started": start,
                    "finished": end,
                    "minutes": (end - start).total_seconds() / 60,
                    "temperature_c": entry.get("temperature_c"),
                    "failed": entry["failures"] > begin["failures"],
                }
            )
    return pd.DataFrame(rows)


def _timestamp(value: str) -> dt.datetime:
    return dt.datetime.strptime(value, "%Y-%m-%dT%H:%M:%SZ").replace(tzinfo=dt.timezone.utc)
