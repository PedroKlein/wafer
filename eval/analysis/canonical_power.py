#!/usr/bin/env python3

from __future__ import annotations

import argparse
import csv
import json
from pathlib import Path

import matplotlib.pyplot as plt
import pandas as pd

from wafer_analysis.paths import find_canonical_batch
from wafer_analysis.plots import SYSTEM_COLORS, save_figure, setup_thesis_style
from wafer_analysis.power import clip_to_window, load_telemetry, summarize_power


def read_message_count(result: Path) -> int | None:
    percentiles = result / "percentiles.json"
    if percentiles.is_file():
        return int(json.loads(percentiles.read_text()).get("total_count", 0))
    subscriber = result / "subscriber-metadata.json"
    if subscriber.is_file():
        return int(json.loads(subscriber.read_text()).get("total_recorded", 0))
    sequence = result / "sequence.csv"
    if not sequence.is_file():
        return None
    with sequence.open(newline="") as stream:
        reader = csv.DictReader(stream)
        row = next(reader, None)
    if row is None or "total_received" not in row:
        return None
    return int(row["total_received"])


def read_measurement_window(result: Path) -> tuple[int, int]:
    path = result / "measurement-window.json"
    if not path.is_file():
        raise ValueError(f"missing measurement window: {path}")
    window = json.loads(path.read_text())
    return int(window["started_ns"]), int(window["finished_ns"])


def collect(batch_id: str, experiments: list[str]) -> pd.DataFrame:
    rows = []
    for experiment in experiments:
        batch = find_canonical_batch(experiment, batch_id)
        for telemetry in sorted(batch.rglob("pi-telemetry.csv")):
            metadata = json.loads((telemetry.parent / "metadata.json").read_text())
            messages = read_message_count(telemetry.parent)
            started_ns, finished_ns = read_measurement_window(telemetry.parent)
            samples = clip_to_window(
                load_telemetry(telemetry), started_ns, finished_ns
            )
            summary = summarize_power(samples, messages=messages)
            rows.append(
                {
                    "experiment": experiment,
                    "system": metadata.get("system", "wafer"),
                    "condition": metadata.get("condition", "default"),
                    "run": telemetry.parent.name,
                    **summary,
                }
            )
    if not rows:
        raise ValueError("canonical batch contains no valid PMIC telemetry")
    return pd.DataFrame(rows)


def render(data: pd.DataFrame, output: Path) -> None:
    output.mkdir(parents=True, exist_ok=True)
    data.to_csv(output / "pmic-energy-summary.csv", index=False)
    (output / "pmic-energy-summary.tex").write_text(
        data.groupby("system")[["mean_proxy_watts", "proxy_energy_j", "proxy_energy_per_message_j"]]
        .median()
        .to_latex(float_format="%.6f")
    )

    setup_thesis_style()
    systems = [system for system in ("wafer", "native", "ekuiper") if system in set(data.system)]
    values = [data.loc[data.system == system, "mean_proxy_watts"].values for system in systems]
    fig, ax = plt.subplots(figsize=(6, 4))
    labels = {"wafer": "WAFER", "native": "Native", "ekuiper": "eKuiper"}
    boxes = ax.boxplot(values, tick_labels=[labels[system] for system in systems], patch_artist=True, showfliers=True)
    for patch, system in zip(boxes["boxes"], systems):
        patch.set_facecolor(SYSTEM_COLORS[labels[system]])
    ax.set_ylabel("PMIC internal-rail proxy power (W)")
    ax.set_title(f"Raspberry Pi 5 energy proxy (N={len(data)} runs)")
    ax.text(
        0.5,
        -0.22,
        "Not total USB-C input power; excludes direct 5 V/USB loads and conversion losses.",
        transform=ax.transAxes,
        ha="center",
        fontsize=8,
    )
    save_figure(fig, "power/pmic_proxy", directory=str(output))


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--batch-id", required=True)
    parser.add_argument("--experiments", default="e-perf-1")
    parser.add_argument("--output", type=Path, required=True)
    args = parser.parse_args()
    data = collect(args.batch_id, [value.strip() for value in args.experiments.split(",")])
    render(data, args.output)
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
