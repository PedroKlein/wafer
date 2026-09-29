"""Host-side evidence: per-core CPU use, attempt ledger and batch progress."""

from __future__ import annotations

import datetime as dt
import json
import re
from pathlib import Path

import pandas as pd

_BUSY_FIELDS = ("user", "nice", "system", "irq", "softirq", "steal")
_TOTAL_FIELDS = (*_BUSY_FIELDS, "idle", "iowait")
_LEAF = re.compile(r"run-(\d+)(?:-attempt-(\d+))?")


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


def attempt_ledger(batches: dict[str, Path]) -> pd.DataFrame:
    """Runs, attempts, passes, failed attempts and retried runs per experiment."""
    rows = []
    for experiment, batch in batches.items():
        attempts: dict[tuple[str, int], list[bool]] = {}
        for status_path in sorted(batch.rglob("canonical-status.json")):
            match = _LEAF.fullmatch(status_path.parent.name)
            if match is None:
                continue
            condition = "/".join(status_path.parent.relative_to(batch).parts[:-1])
            passed = json.loads(status_path.read_text()).get("status") == "passed"
            attempts.setdefault((condition, int(match.group(1))), []).append(passed)
        if not attempts:
            continue
        outcomes = [outcome for run in attempts.values() for outcome in run]
        rows.append(
            {
                "experiment": experiment,
                "runs": len(attempts),
                "attempts": len(outcomes),
                "passed_runs": sum(any(run) for run in attempts.values()),
                "failed_attempts": outcomes.count(False),
                "retried_runs": sum(len(run) > 1 for run in attempts.values()),
            }
        )
    return pd.DataFrame(rows)


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
