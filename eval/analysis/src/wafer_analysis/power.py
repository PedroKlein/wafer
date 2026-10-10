from __future__ import annotations

import csv
import json
from pathlib import Path

MEASUREMENT_LABEL = "Raspberry Pi 5 PMIC internal-rail proxy"
MEASUREMENT_LABELS = {
    "rpi5-pmic-internal-rail-proxy": MEASUREMENT_LABEL,
    "jetson-ina3221-rail-proxy": "Jetson INA3221 rail proxy",
    "x86-rapl-package-energy": "x86 RAPL package power",
}
# Two periods of the sampler's default 1 s interval: the runner may stop the sampler
# just before the window's recorded end.
MAX_EDGE_GAP_NS = 2_000_000_000


def measurement_label(leaf: Path) -> str:
    """The human label of the power quantity `power-boundary.json` declares for a leaf."""
    path = leaf / "power-boundary.json"
    try:
        measurement = json.loads(path.read_text())["measurement"]
    except (OSError, ValueError, KeyError, TypeError) as error:
        raise ValueError(f"malformed power boundary: {path}: {error}") from error
    if measurement not in MEASUREMENT_LABELS:
        raise ValueError(f"no power measurement in {path}: {measurement}")
    return MEASUREMENT_LABELS[measurement]


def load_telemetry(path: Path) -> list[dict[str, float | int | str]]:
    """The samples of a leaf's `pi-telemetry.csv`, in the power quantity the leaf declares.

    On x86 `rail_proxy_watts` adds every top-level RAPL zone, and `psys` already contains
    the package, so an x86 leaf takes its watts from the package rows of `pmic-rails.csv`.
    """
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
    leaf = path.parent
    x86 = MEASUREMENT_LABELS["x86-rapl-package-energy"]
    if (leaf / "power-boundary.json").is_file() and measurement_label(leaf) == x86:
        package = _package_watts(leaf / "pmic-rails.csv")
        rows = [
            {**row, "rail_proxy_watts": package[row["timestamp_ns"]]}
            for row in rows
            if row["timestamp_ns"] in package
        ]
    return rows


def _package_watts(path: Path) -> dict[int, float]:
    watts: dict[int, float] = {}
    with path.open(newline="") as stream:
        for row in csv.DictReader(stream):
            if row["rail"].startswith("package"):
                timestamp_ns = int(row["timestamp_ns"])
                watts[timestamp_ns] = watts.get(timestamp_ns, 0.0) + float(row["power_w"])
    return watts


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
    if (
        int(ordered[0]["timestamp_ns"]) - started_ns > MAX_EDGE_GAP_NS
        or finished_ns - int(ordered[-1]["timestamp_ns"]) > MAX_EDGE_GAP_NS
    ):
        raise ValueError("power telemetry does not cover the measurement window")

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
    measurement: str = MEASUREMENT_LABEL,
) -> dict[str, float | int | str | bool | None]:
    if not samples:
        raise ValueError("power telemetry contains no samples")
    if idle_watts and measurement != MEASUREMENT_LABEL:
        raise ValueError(f"the idle baseline is a {MEASUREMENT_LABEL} reading, not {measurement}")
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
        "measurement": measurement,
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


def read_idle_watts(path: Path) -> float:
    """Median idle proxy watts from a usable, unthrottled idle-baseline.json."""
    baseline = json.loads(path.read_text())
    if baseline.get("experiment") != "idle-baseline":
        raise ValueError(f"{path} is not an idle baseline")
    if baseline.get("usable") is not True or baseline.get("throttled") is not False:
        raise ValueError(f"{path} was not taken on an idle, unthrottled host")
    return float(baseline["idle_proxy_watts_median"])
