#!/usr/bin/env python3
"""Host power and thermal sampler that runs beside a canonical leaf.

The loop, the output files and their columns are the same on every host; a
backend supplies the power rails, the throttle probe and the thermal zone.
`power-boundary.json` says which quantity the backend measures, because Pi
PMIC rails, Jetson INA3221 rails and RAPL package energy are not the same
thing. The `throttled` column is `0x0` when the host is not throttled,
whatever the backend, so every reader keeps that single convention.
"""

from __future__ import annotations

import argparse
import csv
import json
import platform
import re
import shutil
import signal
import subprocess
import sys
import time
from pathlib import Path

from proc_telemetry import pin_to

NOT_THROTTLED = "0x0"
PINNED_CLOCK_TOLERANCE = 0.95
POWER_SOURCE = "https://github.com/raspberrypi/documentation/blob/master/documentation/asciidoc/computers/raspberry-pi/power-supplies.adoc"
BOUNDARY = {
    "measurement": "rpi5-pmic-internal-rail-proxy",
    "is_total_input_power": False,
    "excludes": [
        "USB current",
        "devices connected directly to 5V",
        "PMIC conversion losses",
        "power-supply conversion losses",
    ],
    "source": POWER_SOURCE,
}
JETSON_BOUNDARY = {
    "measurement": "jetson-ina3221-rail-proxy",
    "is_total_input_power": False,
    "excludes": [
        "carrier-board regulators upstream of the monitored rails",
        "USB and PCIe devices powered from the carrier board",
    ],
    "source": "https://docs.nvidia.com/jetson/archives/r36.3/DeveloperGuide/SD/PlatformPowerAndPerformance.html",
}
JETSON_INPUT_RAILS = ("VDD_IN", "POM_5V_IN")
X86_RAPL_BOUNDARY = {
    "measurement": "x86-rapl-package-energy",
    "is_total_input_power": False,
    "excludes": [
        "DRAM outside the DRAM domain",
        "chipset, storage, network and peripherals",
        "voltage regulators and the power supply",
    ],
    "source": "https://www.kernel.org/doc/html/latest/power/powercap/powercap.html",
}
X86_NO_POWER_BOUNDARY = {
    "measurement": "unavailable",
    "is_total_input_power": False,
    "excludes": ["everything: this host exposes no power counters"],
    "source": "https://www.kernel.org/doc/html/latest/power/powercap/powercap.html",
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


def read_text(path: Path) -> str | None:
    try:
        return path.read_text().strip()
    except OSError:
        return None


def read_int(path: Path) -> int | None:
    text = read_text(path)
    if text is None:
        return None
    try:
        return int(text)
    except ValueError:
        return None


def _cpu_ids(spec: str | None) -> list[int]:
    if not spec:
        return []
    cpus = []
    for part in spec.split(","):
        if "-" in part:
            low, high = (int(value) for value in part.split("-", 1))
            cpus.extend(range(low, high + 1))
        else:
            cpus.append(int(part))
    return cpus


def cpufreq_dirs(sysroot: Path) -> list[Path]:
    cpu_root = sysroot / "sys/devices/system/cpu"
    online = _cpu_ids(read_text(cpu_root / "online"))
    paths = (
        [cpu_root / f"cpu{cpu}/cpufreq" for cpu in online]
        if online
        else cpu_root.glob("cpu[0-9]*/cpufreq")
    )
    return sorted(
        (path for path in paths if path.is_dir()),
        key=lambda path: int(path.parent.name[3:]),
    )


def read_cpu_frequency_hz(sysroot: Path = Path("/")) -> int:
    values = []
    for cpufreq in cpufreq_dirs(sysroot):
        value = read_int(cpufreq / "scaling_cur_freq")
        if value is not None:
            values.append(value * 1000)
    return max(values, default=0)


def read_governor(sysroot: Path = Path("/")) -> str:
    values = sorted(
        {
            governor
            for cpufreq in cpufreq_dirs(sysroot)
            if (governor := read_text(cpufreq / "scaling_governor")) is not None
        }
    )
    return "+".join(values) if values else "unknown"


def thermal_zones(sysroot: Path) -> list[Path]:
    return sorted(
        sysroot.glob("sys/class/thermal/thermal_zone[0-9]*"),
        key=lambda path: int(path.name[12:]),
    )


def hwmon_dirs(sysroot: Path, names: tuple[str, ...]) -> list[Path]:
    return sorted(
        hwmon
        for hwmon in sysroot.glob("sys/class/hwmon/hwmon[0-9]*")
        if read_text(hwmon / "name") in names
    )


def read_temperature_millicelsius(
    sysroot: Path = Path("/"),
    zone_types: tuple[str, ...] = (),
    hwmon_names: tuple[str, ...] = (),
) -> int:
    """The first readable source: a zone of a wanted type, a wanted hwmon, any zone."""
    zones = thermal_zones(sysroot)
    by_type = {read_text(zone / "type"): zone for zone in zones}
    candidates = [by_type[zone_type] for zone_type in zone_types if zone_type in by_type]
    for zone in candidates:
        value = read_int(zone / "temp")
        if value is not None:
            return value
    for hwmon in hwmon_dirs(sysroot, hwmon_names):
        value = read_int(hwmon / "temp1_input")
        if value is not None:
            return value
    for zone in zones:
        value = read_int(zone / "temp")
        if value is not None:
            return value
    return 0


def cpus_below_pinned_clock(sysroot: Path) -> list[str]:
    below = []
    for cpufreq in cpufreq_dirs(sysroot):
        current = read_int(cpufreq / "scaling_cur_freq")
        maximum = read_int(cpufreq / "scaling_max_freq")
        if current is None or not maximum:
            continue
        if current < maximum * PINNED_CLOCK_TOLERANCE:
            below.append(f"{cpufreq.parent.name}-below-pinned-clock")
    return below


def x86_clock_policy_reasons(sysroot: Path, expected_cpus: set[str]) -> list[str]:
    policies = {path.parent.name: path for path in cpufreq_dirs(sysroot)}
    reasons = [f"{cpu}-clock-policy-missing" for cpu in sorted(expected_cpus - policies.keys())]
    reasons.extend(f"{cpu}-clock-policy-unexpected" for cpu in sorted(policies.keys() - expected_cpus))
    for cpu in sorted(expected_cpus & policies.keys()):
        policy = policies[cpu]
        if read_text(policy / "scaling_governor") != "performance":
            reasons.append(f"{cpu}-governor-not-performance")
        minimum = read_int(policy / "scaling_min_freq")
        maximum = read_int(policy / "scaling_max_freq")
        if not minimum or not maximum:
            reasons.append(f"{cpu}-clock-limits-unreadable")
        elif minimum != maximum:
            reasons.append(f"{cpu}-clock-not-pinned")
    return reasons


def throttle_token(reasons: list[str]) -> str:
    return "+".join(reasons) if reasons else NOT_THROTTLED


class PiBackend:
    name = "pi"
    boundary = BOUNDARY
    zone_types = ("cpu-thermal",)
    hwmon_names: tuple[str, ...] = ()

    def __init__(self, sysroot: Path) -> None:
        self.sysroot = sysroot

    def rails(self, timestamp_ns: int) -> list[dict[str, float | str]]:
        return parse_pmic(command_output(["vcgencmd", "pmic_read_adc"]))

    def throttled(self) -> str:
        return command_output(["vcgencmd", "get_throttled"]).removeprefix("throttled=")


class JetsonBackend:
    name = "jetson"
    boundary = JETSON_BOUNDARY
    zone_types = ("cpu-thermal", "CPU-therm")
    hwmon_names: tuple[str, ...] = ()

    def __init__(self, sysroot: Path) -> None:
        self.sysroot = sysroot

    def rails(self, timestamp_ns: int) -> list[dict[str, float | str]]:
        rails = []
        for hwmon in hwmon_dirs(self.sysroot, ("ina3221",)):
            for label_path in sorted(hwmon.glob("in[0-9]*_label")):
                channel = label_path.name[2:].split("_", 1)[0]
                label = read_text(label_path)
                millivolts = read_int(hwmon / f"in{channel}_input")
                milliamps = read_int(hwmon / f"curr{channel}_input")
                if not label or millivolts is None or milliamps is None:
                    continue
                voltage = millivolts / 1000
                current = milliamps / 1000
                rails.append(
                    {
                        "rail": label,
                        "current_a": current,
                        "voltage_v": voltage,
                        "power_w": voltage * current,
                    }
                )
        if not rails:
            raise OSError("no INA3221 rails under /sys/class/hwmon")
        return rails

    def throttled(self) -> str:
        return throttle_token(cpus_below_pinned_clock(self.sysroot))


class X86Backend:
    name = "x86"
    zone_types = ("x86_pkg_temp",)
    hwmon_names = ("k10temp", "coretemp")

    def __init__(self, sysroot: Path) -> None:
        self.sysroot = sysroot
        self.domains = sorted(
            domain
            for domain in sysroot.glob("sys/class/powercap/intel-rapl:[0-9]*")
            if ":" not in domain.name.split("intel-rapl:", 1)[1]
            and read_int(domain / "energy_uj") is not None
        )
        self.boundary = X86_RAPL_BOUNDARY if self.domains else X86_NO_POWER_BOUNDARY
        self.previous: dict[str, tuple[int, int]] = {}
        self.clock_policy_cpus = {path.parent.name for path in cpufreq_dirs(sysroot)}
        self.throttle_counts: dict[str, int] = {}

    def rails(self, timestamp_ns: int) -> list[dict[str, float | str]]:
        rails = []
        for domain in self.domains:
            energy = read_int(domain / "energy_uj")
            if energy is None:
                continue
            label = read_text(domain / "name") or domain.name
            last = self.previous.get(label)
            self.previous[label] = (timestamp_ns, energy)
            if last is None:
                continue
            last_ns, last_energy = last
            if timestamp_ns <= last_ns:
                continue
            delta = energy - last_energy
            if delta < 0:
                wrap = read_int(domain / "max_energy_range_uj")
                if wrap is None:
                    continue
                delta += wrap
            rails.append(
                {
                    "rail": label,
                    "current_a": "",
                    "voltage_v": "",
                    "power_w": delta / (timestamp_ns - last_ns) * 1000,
                }
            )
        return rails

    def throttled(self) -> str:
        reasons = x86_clock_policy_reasons(self.sysroot, self.clock_policy_cpus)
        counters = [
            (counter, f"{counter.parent.parent.name}-thermal-throttle")
            for counter in sorted(
                self.sysroot.glob(
                    "sys/devices/system/cpu/cpu[0-9]*/thermal_throttle/core_throttle_count"
                )
            )
        ]
        package = self.sysroot / "sys/devices/system/cpu/cpu0/thermal_throttle/package_throttle_count"
        if package.is_file():
            counters.append((package, "package-thermal-throttle"))
        for counter, reason in counters:
            count = read_int(counter)
            if count is None:
                continue
            key = str(counter)
            previous = self.throttle_counts.get(key)
            if previous is not None and count > previous:
                reasons.append(reason)
            self.throttle_counts[key] = count
        return throttle_token(reasons)


BACKENDS = {"pi": PiBackend, "jetson": JetsonBackend, "x86": X86Backend}


def detect_backend_name(sysroot: Path = Path("/"), machine: str | None = None) -> str:
    if shutil.which("vcgencmd") is not None:
        return "pi"
    if (sysroot / "etc/nv_tegra_release").is_file():
        return "jetson"
    if (machine or platform.machine()) == "x86_64":
        return "x86"
    raise OSError("no telemetry backend for this host: no vcgencmd, no L4T release and not x86_64")


def make_backend(name: str, sysroot: Path = Path("/")) -> PiBackend | JetsonBackend | X86Backend:
    if name == "auto":
        name = detect_backend_name(sysroot)
    return BACKENDS[name](sysroot)


def host_snapshot() -> tuple[int, str]:
    """Current temperature and throttle state of this host, (0, "unknown") without a backend."""
    try:
        backend = make_backend("auto")
        temperature = read_temperature_millicelsius(
            backend.sysroot, backend.zone_types, backend.hwmon_names
        )
        return temperature, backend.throttled()
    except (OSError, KeyError, subprocess.CalledProcessError):
        return 0, "unknown"


def sample(
    backend: PiBackend | JetsonBackend | X86Backend,
) -> tuple[dict[str, int | float | str], list[dict[str, float | str]]]:
    timestamp_ns = time.time_ns()
    rails = backend.rails(timestamp_ns)
    summary = {
        "timestamp_ns": timestamp_ns,
        "temperature_millicelsius": read_temperature_millicelsius(
            backend.sysroot, backend.zone_types, backend.hwmon_names
        ),
        "cpu_frequency_hz": read_cpu_frequency_hz(backend.sysroot),
        "governor": read_governor(backend.sysroot),
        "throttled": backend.throttled(),
        "rail_proxy_watts": proxy_watts(rails),
    }
    return summary, rails


def proxy_watts(rails: list[dict[str, float | str]]) -> float:
    """Sum the rails, or take the Jetson module input alone, which already feeds the others."""
    inputs = [rail for rail in rails if rail["rail"] in JETSON_INPUT_RAILS]
    return sum(float(rail["power_w"]) for rail in inputs or rails)


def write_error(output_dir: Path, error: BaseException) -> None:
    (output_dir / "telemetry-error.json").write_text(
        json.dumps({"error": str(error), "timestamp_ns": time.time_ns()}) + "\n"
    )


def run(
    output_dir: Path,
    interval_secs: float,
    backend_name: str = "auto",
    sysroot: Path = Path("/"),
    pin_cpus: str = "",
) -> int:
    output_dir.mkdir(parents=True, exist_ok=True)
    try:
        if pin_cpus:
            pin_to(pin_cpus)
        backend = make_backend(backend_name, sysroot)
    except (OSError, KeyError) as error:
        write_error(output_dir, error)
        return 1
    (output_dir / "power-boundary.json").write_text(
        json.dumps({**backend.boundary, "backend": backend.name}, indent=2) + "\n"
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
                summary, rails = sample(backend)
            except (OSError, subprocess.CalledProcessError) as error:
                write_error(output_dir, error)
                return 1
            summary_writer.writerow(summary)
            for rail in rails:
                rails_writer.writerow({"timestamp_ns": summary["timestamp_ns"], **rail})
            summary_file.flush()
            rails_file.flush()
            time.sleep(max(0.0, interval_secs - (time.monotonic() - started)))
    return 0


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    parser.add_argument("output_dir", type=Path)
    parser.add_argument("interval_secs", type=float, nargs="?", default=1.0)
    parser.add_argument("--backend", choices=("auto", *BACKENDS), default="auto")
    parser.add_argument("--sysroot", type=Path, default=Path("/"), help=argparse.SUPPRESS)
    parser.add_argument("--pin-cpus", default="", help="CPU list the sampler pins itself to")
    args = parser.parse_args(argv)
    return run(args.output_dir, args.interval_secs, args.backend, args.sysroot, args.pin_cpus)


if __name__ == "__main__":
    raise SystemExit(main())
