#!/usr/bin/env python3
"""Compare E-Perf-1 WAFER runs with the host sidecars on and off.

Reads <root>/{on,off}/pair-NN/ leaves written by run-rpi5-instrument-ab.sh and
writes <root>/instrument-ab.json. Differences are paired within a pair and
relative to the sidecars-off run. The tolerance is the same 5% used for the
Preview 2 async A/B in RESULT-CONTRACT.md.
"""

from __future__ import annotations

import json
import random
import statistics
import sys
from pathlib import Path

ARMS = ("on", "off")
METRICS = ("p50_ns", "p95_ns", "p99_ns", "achieved_msg_s")
TOLERANCE_PERCENT = 5.0
MAX_SAMPLER_CORE_FRACTION = 0.01
BOOTSTRAP_RESAMPLES = 10_000
BOOTSTRAP_SEED = 1729
EXPECTED_MESSAGES = 60_000


def leaf_metrics(leaf: Path) -> dict[str, float]:
    """Loss is counted against the 60,000 messages the run offers after warmup,
    so messages missing at the start or end of the run count, not only gaps."""
    subscriber = json.loads((leaf / "subscriber-metadata.json").read_text())
    window = json.loads((leaf / "measurement-window.json").read_text())
    seconds = (int(window["finished_ns"]) - int(window["started_ns"])) / 1e9
    sequence = subscriber.get("sequence", {})
    received_unique = int(sequence.get("received_unique", subscriber["total_recorded"]))
    metrics: dict[str, float] = {
        "p50_ns": float(subscriber["latency_p50_ns"]),
        "p95_ns": float(subscriber["latency_p95_ns"]),
        "p99_ns": float(subscriber["latency_p99_ns"]),
        "achieved_msg_s": int(subscriber["total_recorded"]) / seconds if seconds > 0 else 0.0,
        "received_unique": float(received_unique),
        "lost": float(max(0, EXPECTED_MESSAGES - received_unique)),
        "duplicates": float(sequence.get("total_duplicates", 0)),
    }
    sidecar = leaf / "host-sidecar.json"
    if sidecar.is_file():
        receipt = json.loads(sidecar.read_text())
        elapsed = (int(receipt["finished_unix_epoch_ns"]) - int(receipt["started_unix_epoch_ns"])) / 1e9
        metrics["sampler_core_fraction"] = float(receipt["sampler_cpu_seconds"]) / elapsed if elapsed > 0 else 0.0
    return metrics


def bootstrap_median_ci(values: list[float]) -> list[float]:
    rng = random.Random(BOOTSTRAP_SEED)
    medians = sorted(
        statistics.median(rng.choices(values, k=len(values))) for _ in range(BOOTSTRAP_RESAMPLES)
    )
    return [medians[int(0.025 * BOOTSTRAP_RESAMPLES)], medians[int(0.975 * BOOTSTRAP_RESAMPLES) - 1]]


def check_arm(leaf: Path, arm: str) -> list[str]:
    problems = []
    for name in ("subscriber-metadata.json", "measurement-window.json"):
        if not (leaf / name).is_file():
            problems.append(f"missing {name}")
    for name in ("telemetry-error.json", "host-sidecar-error.json"):
        if (leaf / name).exists():
            problems.append(f"sidecar failed: {name}")
    has_sidecar = (leaf / "host-sidecar.json").is_file() or (leaf / "pi-telemetry.csv").is_file()
    if arm == "on" and not (leaf / "host-sidecar.json").is_file():
        problems.append("sidecars-on run has no host-sidecar.json")
    if arm == "off" and has_sidecar:
        problems.append("sidecars-off run carries sidecar output")
    return problems


def analyze(root: Path) -> dict[str, object]:
    pair_names = sorted({path.name for arm in ARMS for path in (root / arm).glob("pair-*") if path.is_dir()})
    pairs = []
    rejected = []
    for name in pair_names:
        leaves = {arm: root / arm / name for arm in ARMS}
        problems = [f"{arm}: {problem}" for arm in ARMS for problem in check_arm(leaves[arm], arm)]
        if problems:
            rejected.append({"pair": name, "problems": problems})
            continue
        on = leaf_metrics(leaves["on"])
        off = leaf_metrics(leaves["off"])
        empty = [f"{arm}: {metric} is zero" for arm, values in (("on", on), ("off", off)) for metric in METRICS if values[metric] <= 0]
        if empty:
            rejected.append({"pair": name, "problems": empty})
            continue
        pairs.append(
            {
                "pair": name,
                "on": on,
                "off": off,
                "relative_difference_percent": {
                    metric: 100.0 * (on[metric] - off[metric]) / off[metric] for metric in METRICS
                },
            }
        )
    if len(pairs) < 2:
        raise ValueError(f"need at least two complete pairs, found {len(pairs)}")

    summary: dict[str, dict[str, object]] = {}
    for metric in METRICS:
        differences = [pair["relative_difference_percent"][metric] for pair in pairs]
        summary[metric] = {
            "median_relative_difference_percent": statistics.median(differences),
            "bootstrap_95_ci_percent": bootstrap_median_ci(differences),
            "min_percent": min(differences),
            "max_percent": max(differences),
            "median_on": statistics.median(pair["on"][metric] for pair in pairs),
            "median_off": statistics.median(pair["off"][metric] for pair in pairs),
        }
    sampler = [pair["on"]["sampler_core_fraction"] for pair in pairs]
    lossless = all(pair[arm]["lost"] == 0 and pair[arm]["duplicates"] == 0 for pair in pairs for arm in ARMS)
    within_tolerance = all(
        abs(float(summary[metric]["median_relative_difference_percent"])) <= TOLERANCE_PERCENT
        for metric in ("p95_ns", "achieved_msg_s")
    )
    negligible = (
        within_tolerance and lossless and max(sampler) <= MAX_SAMPLER_CORE_FRACTION and not rejected
    )
    return {
        "schema_version": 1,
        "experiment": "instrument-ab",
        "evidence_class": "diagnostic",
        "thesis_evidence": False,
        "workload": "e-perf-1 wafer at 1,000 msg/s",
        "pairs": len(pairs),
        "rejected_pairs": rejected,
        "tolerance_percent": TOLERANCE_PERCENT,
        "rule": (
            "negligible when the median paired relative difference of p95 and achieved rate is within "
            "the tolerance, every run is lossless, the /proc sampler used at most 1% of one core, "
            "and no pair was rejected"
        ),
        "bootstrap": {"statistic": "median", "method": "percentile", "resamples": BOOTSTRAP_RESAMPLES, "seed": BOOTSTRAP_SEED},
        "metrics": summary,
        "sampler_core_fraction_max": max(sampler),
        "lossless": lossless,
        "negligible": negligible,
        "per_pair": pairs,
    }


def main(argv: list[str]) -> int:
    if len(argv) != 2:
        print(f"usage: {argv[0]} <instrument-ab-dir>", file=sys.stderr)
        return 2
    root = Path(argv[1])
    result = analyze(root)
    (root / "instrument-ab.json").write_text(json.dumps(result, indent=2) + "\n")
    p95 = result["metrics"]["p95_ns"]
    print(
        f"instrument A/B: {result['pairs']} pairs, p95 on-vs-off median "
        f"{p95['median_relative_difference_percent']:+.2f}% (CI {p95['bootstrap_95_ci_percent'][0]:+.2f}.."
        f"{p95['bootstrap_95_ci_percent'][1]:+.2f}%), sampler max {100 * result['sampler_core_fraction_max']:.3f}% "
        f"of a core, negligible={result['negligible']}"
    )
    return 0 if result["negligible"] else 1


if __name__ == "__main__":
    sys.exit(main(sys.argv))
