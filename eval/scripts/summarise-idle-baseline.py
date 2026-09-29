#!/usr/bin/env python3
"""Summarise an idle-baseline directory into idle-baseline.json.

Each sample directory holds the two sidecar outputs and a measurement window.
The summary gives the PMIC internal-rail proxy watts per sample and across
samples, plus the SUT-core busy fraction that proves the host really was idle.
"""

from __future__ import annotations

import csv
import importlib.util
import json
import statistics
import sys
from pathlib import Path

POWER_MODULE = Path(__file__).resolve().parents[1] / "analysis/src/wafer_analysis/power.py"


def load_power_module():
    """power.py alone has no third-party imports; the package around it needs numpy."""
    spec = importlib.util.spec_from_file_location("wafer_power", POWER_MODULE)
    assert spec and spec.loader
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


power_module = load_power_module()
clip_to_window = power_module.clip_to_window
load_telemetry = power_module.load_telemetry
summarize_power = power_module.summarize_power

BUSY_COLUMNS = ("user", "nice", "system", "iowait", "irq", "softirq", "steal")
ALL_COLUMNS = (*BUSY_COLUMNS, "idle")
MAX_IDLE_BUSY_FRACTION = 0.02


def parse_cpu_list(text: str) -> set[int]:
    cpus: set[int] = set()
    for part in text.split(","):
        part = part.strip()
        if not part:
            continue
        if "-" in part:
            low, high = part.split("-", 1)
            cpus.update(range(int(low), int(high) + 1))
        else:
            cpus.add(int(part))
    return cpus


def core_busy_fraction(cpu_cores: Path, cpus: set[int]) -> float | None:
    first: dict[int, dict[str, int]] = {}
    last: dict[int, dict[str, int]] = {}
    with cpu_cores.open(newline="") as stream:
        for row in csv.DictReader(stream):
            cpu = int(row["cpu"])
            if cpus and cpu not in cpus:
                continue
            values = {column: int(row[column]) for column in ALL_COLUMNS}
            first.setdefault(cpu, values)
            last[cpu] = values
    busy = sum(last[cpu][c] - first[cpu][c] for cpu in first for c in BUSY_COLUMNS)
    total = sum(last[cpu][c] - first[cpu][c] for cpu in first for c in ALL_COLUMNS)
    return busy / total if total > 0 else None


def summarise_sample(sample: Path) -> dict[str, object]:
    window = json.loads((sample / "measurement-window.json").read_text())
    telemetry = clip_to_window(
        load_telemetry(sample / "pi-telemetry.csv"),
        int(window["started_ns"]),
        int(window["finished_ns"]),
    )
    power = summarize_power(telemetry)
    sidecar = json.loads((sample / "host-sidecar.json").read_text())
    sut_cpus = parse_cpu_list(str(sidecar.get("sut_cpus", "")))
    return {
        "sample": sample.name,
        "duration_s": power["duration_s"],
        "mean_proxy_watts": power["mean_proxy_watts"],
        "max_temperature_c": power["max_temperature_c"],
        "throttled": power["throttled"],
        "sut_core_busy_fraction": core_busy_fraction(sample / "cpu-cores.csv", sut_cpus),
        "sampler_cpu_seconds": sidecar.get("sampler_cpu_seconds"),
    }


REQUIRED_FILES = ("pi-telemetry.csv", "cpu-cores.csv", "host-sidecar.json", "measurement-window.json")


def summarise(root: Path) -> dict[str, object]:
    samples = []
    rejected = []
    for path in sorted(root.glob("sample-*")):
        if not path.is_dir():
            continue
        missing = [name for name in REQUIRED_FILES if not (path / name).is_file()]
        errors = [name for name in ("telemetry-error.json", "host-sidecar-error.json") if (path / name).exists()]
        if missing or errors:
            rejected.append({"sample": path.name, "missing": missing, "errors": errors})
            continue
        samples.append(summarise_sample(path))
    if not samples:
        raise ValueError(f"no complete sample-* directories under {root}")
    watts = [float(sample["mean_proxy_watts"]) for sample in samples]
    busy = [s["sut_core_busy_fraction"] for s in samples if s["sut_core_busy_fraction"] is not None]
    quartiles = statistics.quantiles(watts, n=4) if len(watts) >= 2 else [watts[0]] * 3
    return {
        "schema_version": 1,
        "experiment": "idle-baseline",
        "evidence_class": "diagnostic",
        "thesis_evidence": False,
        "measurement": "Raspberry Pi 5 PMIC internal-rail proxy",
        "is_total_input_power": False,
        "samples": len(samples),
        "rejected_samples": rejected,
        "idle_proxy_watts_median": statistics.median(watts),
        "idle_proxy_watts_iqr": [quartiles[0], quartiles[2]],
        "idle_proxy_watts_min": min(watts),
        "idle_proxy_watts_max": max(watts),
        "max_temperature_c": max(float(sample["max_temperature_c"]) for sample in samples),
        "throttled": any(bool(sample["throttled"]) for sample in samples),
        "sut_core_busy_fraction_max": max(busy) if busy else None,
        "host_idle": bool(busy) and max(busy) <= MAX_IDLE_BUSY_FRACTION,
        "usable": bool(busy) and max(busy) <= MAX_IDLE_BUSY_FRACTION and not rejected,
        "per_sample": samples,
    }


def main(argv: list[str]) -> int:
    if len(argv) != 2:
        print(f"usage: {argv[0]} <idle-baseline-dir>", file=sys.stderr)
        return 2
    root = Path(argv[1])
    summary = summarise(root)
    (root / "idle-baseline.json").write_text(json.dumps(summary, indent=2) + "\n")
    print(
        f"idle baseline: {summary['samples']} samples, median {summary['idle_proxy_watts_median']:.3f} W "
        f"(proxy), SUT cores busy {summary['sut_core_busy_fraction_max']}, host_idle={summary['host_idle']}"
    )
    return 0 if summary["usable"] and not summary["throttled"] else 1


if __name__ == "__main__":
    sys.exit(main(sys.argv))
