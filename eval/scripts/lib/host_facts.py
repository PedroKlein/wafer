"""Platform facts shared by host-facts.json and metadata.json.

Every key is always present. A fact the host does not expose is ``None``, so
verifiers can require the keys on every platform and compare values only
where they exist.
"""

from __future__ import annotations

import re
import subprocess
from collections.abc import Callable
from pathlib import Path

Runner = Callable[[list[str]], str | None]


def run_command(command: list[str]) -> str | None:
    try:
        output = subprocess.check_output(command, text=True, stderr=subprocess.DEVNULL)
    except (OSError, subprocess.CalledProcessError):
        return None
    return output.strip() or None


def _read(path: Path) -> str | None:
    try:
        text = path.read_text().replace("\x00", "").strip()
    except OSError:
        return None
    return text or None


def _cpuinfo_fields(root: Path) -> list[dict[str, str]]:
    text = _read(root / "proc/cpuinfo") or ""
    processors: list[dict[str, str]] = []
    current: dict[str, str] = {}
    for line in text.splitlines():
        if not line.strip():
            if current:
                processors.append(current)
                current = {}
            continue
        key, _, value = line.partition(":")
        current[key.strip()] = value.strip()
    if current:
        processors.append(current)
    return processors


def _cpu_model(root: Path, processors: list[dict[str, str]]) -> str | None:
    for processor in processors:
        for key in ("model name", "Model", "Hardware"):
            if processor.get(key):
                return processor[key]
    compatible = _read(root / "proc/device-tree/compatible")
    if compatible:
        return compatible.split("\x00")[0] or compatible
    return None


def _count_cpus(spec: str | None) -> int | None:
    if not spec:
        return None
    total = 0
    for part in spec.split(","):
        if "-" in part:
            low, high = part.split("-", 1)
            total += int(high) - int(low) + 1
        else:
            total += 1
    return total


def _physical_cores(processors: list[dict[str, str]], online: str | None) -> int | None:
    cores = {
        (processor.get("physical id"), processor.get("core id"))
        for processor in processors
        if "core id" in processor
    }
    if cores:
        return len(cores)
    return _count_cpus(online)


def _hardware_model(root: Path) -> str | None:
    model = _read(root / "proc/device-tree/model")
    if model:
        return model
    vendor = _read(root / "sys/class/dmi/id/sys_vendor")
    product = _read(root / "sys/class/dmi/id/product_name")
    if product:
        return f"{vendor} {product}" if vendor else product
    return None


def _turbo(root: Path) -> str | None:
    no_turbo = _read(root / "sys/devices/system/cpu/intel_pstate/no_turbo")
    if no_turbo is not None:
        return "off" if no_turbo == "1" else "on"
    boost = _read(root / "sys/devices/system/cpu/cpufreq/boost")
    if boost is not None:
        return "on" if boost == "1" else "off"
    return None


def _pstate_driver(root: Path) -> str | None:
    driver = _read(root / "sys/devices/system/cpu/cpu0/cpufreq/scaling_driver")
    for name in ("intel_pstate", "amd_pstate"):
        status = _read(root / "sys/devices/system/cpu" / name / "status")
        if status:
            return f"{name}:{status}"
    return driver


def _os_release(root: Path) -> str | None:
    text = _read(root / "etc/os-release") or ""
    for line in text.splitlines():
        if line.startswith("PRETTY_NAME="):
            return line.partition("=")[2].strip().strip('"') or None
    return None


def _glibc_version(run: Runner) -> str | None:
    output = run(["getconf", "GNU_LIBC_VERSION"])
    if output is None:
        return None
    match = re.search(r"\d+\.\d+", output)
    return match.group(0) if match else None


def _power_mode(run: Runner) -> str | None:
    output = run(["nvpmodel", "-q"])
    if output is None:
        return None
    for line in output.splitlines():
        if line.startswith("NV Power Mode:"):
            return line.partition(":")[2].strip() or None
    return None


def platform_facts(root: Path = Path("/"), run: Runner = run_command) -> dict:
    processors = _cpuinfo_fields(root)
    online = _read(root / "sys/devices/system/cpu/online")
    return {
        "hardware_model": _hardware_model(root),
        "cpu_model": _cpu_model(root, processors),
        "physical_cores": _physical_cores(processors, online),
        "online_cpus": online,
        "smt": _read(root / "sys/devices/system/cpu/smt/control"),
        "turbo": _turbo(root),
        "cpufreq_driver": _pstate_driver(root),
        "os_release": _os_release(root),
        "glibc_version": _glibc_version(run),
        "power_mode": _power_mode(run),
    }
