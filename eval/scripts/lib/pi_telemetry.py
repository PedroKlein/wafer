#!/usr/bin/env python3

from __future__ import annotations

import csv
import json
import re
import signal
import subprocess
import sys
import time
from pathlib import Path

BOUNDARY = {
    "measurement": "rpi5-pmic-internal-rail-proxy",
    "is_total_input_power": False,
    "excludes": [
        "USB current",
        "devices connected directly to 5V",
        "PMIC conversion losses",
        "power-supply conversion losses",
    ],
    "source": "https://github.com/raspberrypi/documentation/blob/master/documentation/asciidoc/computers/raspberry-pi/power-supplies.adoc",
}
RAIL_PATTERN = re.compile(
    r"^\s*(\S+)\s+(current|volt)\(\d+\)=([0-9.]+)(A|V)\s*$"
)


def parse_pmic(text: str) -> list[dict[str, float | str]]:
    values: dict[str, dict[str, float]] = {}
    for line in text.splitlines():
        match = RAIL_PATTERN.match(line)
        if match is None:
            continue
        name, kind, raw_value, _unit = match.groups()
        suffix = "_A" if kind == "current" else "_V"
        if not name.endswith(suffix):
            continue
        base = name.removesuffix(suffix)
        key = "current_a" if kind == "current" else "voltage_v"
        values.setdefault(base, {})[key] = float(raw_value)

    rails = []
    for name, value in sorted(values.items()):
        if "current_a" not in value or "voltage_v" not in value:
            continue
        rails.append(
            {
                "rail": name,
                "current_a": value["current_a"],
                "voltage_v": value["voltage_v"],
                "power_w": value["current_a"] * value["voltage_v"],
            }
        )
    return rails


def command_output(command: list[str]) -> str:
    return subprocess.check_output(command, text=True, stderr=subprocess.DEVNULL).strip()


def read_cpu_frequency_hz() -> int:
    values = []
    for path in Path("/sys/devices/system/cpu").glob("cpu[0-9]*/cpufreq/scaling_cur_freq"):
        try:
            values.append(int(path.read_text().strip()) * 1000)
        except (OSError, ValueError):
            continue
    return max(values, default=0)


def read_governor() -> str:
    values = sorted(
        {
            path.read_text().strip()
            for path in Path("/sys/devices/system/cpu").glob(
                "cpu[0-9]*/cpufreq/scaling_governor"
            )
        }
    )
    return "+".join(values) if values else "unknown"


def read_temperature_millicelsius() -> int:
    try:
        return int(Path("/sys/class/thermal/thermal_zone0/temp").read_text().strip())
    except (OSError, ValueError):
        return 0


def sample() -> tuple[dict[str, int | float | str], list[dict[str, float | str]]]:
    timestamp_ns = time.time_ns()
    rails = parse_pmic(command_output(["vcgencmd", "pmic_read_adc"]))
    throttled = command_output(["vcgencmd", "get_throttled"]).removeprefix(
        "throttled="
    )
    summary = {
        "timestamp_ns": timestamp_ns,
        "temperature_millicelsius": read_temperature_millicelsius(),
        "cpu_frequency_hz": read_cpu_frequency_hz(),
        "governor": read_governor(),
        "throttled": throttled,
        "rail_proxy_watts": sum(float(rail["power_w"]) for rail in rails),
    }
    return summary, rails


def run(output_dir: Path, interval_secs: float) -> int:
    output_dir.mkdir(parents=True, exist_ok=True)
    (output_dir / "power-boundary.json").write_text(
        json.dumps(BOUNDARY, indent=2) + "\n"
    )
    stop = False

    def request_stop(_signum: int, _frame: object) -> None:
        nonlocal stop
        stop = True

    signal.signal(signal.SIGTERM, request_stop)
    signal.signal(signal.SIGINT, request_stop)
    with (output_dir / "pi-telemetry.csv").open("w", newline="") as summary_file, (
        output_dir / "pmic-rails.csv"
    ).open("w", newline="") as rails_file:
        summary_writer = csv.DictWriter(
            summary_file,
            fieldnames=[
                "timestamp_ns",
                "temperature_millicelsius",
                "cpu_frequency_hz",
                "governor",
                "throttled",
                "rail_proxy_watts",
            ],
        )
        rails_writer = csv.DictWriter(
            rails_file,
            fieldnames=["timestamp_ns", "rail", "current_a", "voltage_v", "power_w"],
        )
        summary_writer.writeheader()
        rails_writer.writeheader()
        while not stop:
            started = time.monotonic()
            try:
                summary, rails = sample()
            except (OSError, subprocess.CalledProcessError) as error:
                (output_dir / "telemetry-error.json").write_text(
                    json.dumps({"error": str(error), "timestamp_ns": time.time_ns()})
                    + "\n"
                )
                return 1
            summary_writer.writerow(summary)
            for rail in rails:
                rails_writer.writerow({"timestamp_ns": summary["timestamp_ns"], **rail})
            summary_file.flush()
            rails_file.flush()
            time.sleep(max(0.0, interval_secs - (time.monotonic() - started)))
    return 0


def main() -> int:
    if len(sys.argv) not in {2, 3}:
        print(f"usage: {sys.argv[0]} <output-dir> [interval-secs]", file=sys.stderr)
        return 2
    interval = float(sys.argv[2]) if len(sys.argv) == 3 else 1.0
    return run(Path(sys.argv[1]), interval)


if __name__ == "__main__":
    raise SystemExit(main())
