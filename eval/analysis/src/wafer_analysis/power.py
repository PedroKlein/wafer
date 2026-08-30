from __future__ import annotations

import csv
from pathlib import Path

MEASUREMENT_LABEL = "Raspberry Pi 5 PMIC internal-rail proxy"


def load_telemetry(path: Path) -> list[dict[str, float | int | str]]:
    with path.open(newline="") as stream:
        rows = []
        for row in csv.DictReader(stream):
            rows.append(
                {
                    "timestamp_ns": int(row["timestamp_ns"]),
                    "temperature_millicelsius": int(row["temperature_millicelsius"]),
                    "cpu_frequency_hz": int(row["cpu_frequency_hz"]),
                    "governor": row["governor"],
                    "throttled": row["throttled"],
                    "rail_proxy_watts": float(row["rail_proxy_watts"]),
                }
            )
    return rows


def summarize_power(
    samples: list[dict[str, float | int | str]],
    idle_watts: float = 0.0,
    messages: int | None = None,
) -> dict[str, float | int | str | bool | None]:
    if not samples:
        raise ValueError("power telemetry contains no samples")
    ordered = sorted(samples, key=lambda sample: int(sample["timestamp_ns"]))
    energy_j = 0.0
    for left, right in zip(ordered, ordered[1:]):
        elapsed = (int(right["timestamp_ns"]) - int(left["timestamp_ns"])) / 1e9
        watts = (float(left["rail_proxy_watts"]) + float(right["rail_proxy_watts"])) / 2
        energy_j += watts * elapsed
    duration_s = (int(ordered[-1]["timestamp_ns"]) - int(ordered[0]["timestamp_ns"])) / 1e9
    mean_watts = energy_j / duration_s if duration_s > 0 else float(ordered[0]["rail_proxy_watts"])
    adjusted_watts = max(0.0, mean_watts - idle_watts)
    adjusted_energy_j = adjusted_watts * duration_s
    return {
        "measurement": MEASUREMENT_LABEL,
        "is_total_input_power": False,
        "samples": len(ordered),
        "duration_s": duration_s,
        "mean_proxy_watts": mean_watts,
        "proxy_energy_j": energy_j,
        "idle_proxy_watts": idle_watts,
        "idle_adjusted_proxy_watts": adjusted_watts,
        "idle_adjusted_proxy_energy_j": adjusted_energy_j,
        "proxy_energy_per_message_j": adjusted_energy_j / messages if messages else None,
        "max_temperature_c": max(int(sample["temperature_millicelsius"]) for sample in ordered) / 1000,
        "throttled": any(str(sample["throttled"]) != "0x0" for sample in ordered),
    }
