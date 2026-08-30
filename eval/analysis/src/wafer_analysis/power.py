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


def clip_to_window(
    samples: list[dict[str, float | int | str]],
    started_ns: int,
    finished_ns: int,
) -> list[dict[str, float | int | str]]:
    if not samples:
        raise ValueError("power telemetry contains no samples")
    if finished_ns <= started_ns:
        raise ValueError("measurement window must finish after it starts")
    ordered = sorted(samples, key=lambda sample: int(sample["timestamp_ns"]))
    started_ns = max(started_ns, int(ordered[0]["timestamp_ns"]))
    finished_ns = min(finished_ns, int(ordered[-1]["timestamp_ns"]))
    if finished_ns <= started_ns:
        raise ValueError("measurement window does not overlap power telemetry")

    def boundary(timestamp_ns: int) -> dict[str, float | int | str]:
        left = ordered[0]
        right = ordered[-1]
        for sample in ordered:
            sample_ns = int(sample["timestamp_ns"])
            if sample_ns <= timestamp_ns:
                left = sample
            if sample_ns >= timestamp_ns:
                right = sample
                break
        result = dict(
            left
            if timestamp_ns - int(left["timestamp_ns"])
            <= int(right["timestamp_ns"]) - timestamp_ns
            else right
        )
        left_ns = int(left["timestamp_ns"])
        right_ns = int(right["timestamp_ns"])
        if left_ns < timestamp_ns < right_ns:
            ratio = (timestamp_ns - left_ns) / (right_ns - left_ns)
            result["rail_proxy_watts"] = float(left["rail_proxy_watts"]) + ratio * (
                float(right["rail_proxy_watts"]) - float(left["rail_proxy_watts"])
            )
        result["timestamp_ns"] = timestamp_ns
        return result

    within = [
        sample
        for sample in ordered
        if started_ns < int(sample["timestamp_ns"]) < finished_ns
    ]
    return [boundary(started_ns), *within, boundary(finished_ns)]


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
