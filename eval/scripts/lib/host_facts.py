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

PLATFORM_KEYS = (
    "hardware_model",
    "cpu_model",
    "physical_cores",
    "online_cpus",
    "smt",
    "turbo",
    "cpufreq_driver",
    "os_release",
    "glibc_version",
    "power_mode",
)
CPU_POLICY_KEYS = ("isolated_cpus", "housekeeping_cpus", "irq_default_cpus")


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
    try:
        compatible = (root / "proc/device-tree/compatible").read_bytes()
    except OSError:
        return None
    first = compatible.split(b"\x00", 1)[0].decode(errors="replace").strip()
    return first or None


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


def _count_cpus(spec: str | None) -> int | None:
    cpus = _cpu_ids(spec)
    return len(cpus) if cpus else None


def cpu_governors(root: Path = Path("/")) -> list[str]:
    cpu_root = root / "sys/devices/system/cpu"
    online = _cpu_ids(_read(cpu_root / "online"))
    paths = (
        [cpu_root / f"cpu{cpu}/cpufreq/scaling_governor" for cpu in online]
        if online
        else cpu_root.glob("cpu[0-9]*/cpufreq/scaling_governor")
    )
    return sorted({governor for path in paths if (governor := _read(path)) is not None})


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
    for name in ("intel_pstate", "amd_pstate"):
        status = _read(root / "sys/devices/system/cpu" / name / "status")
        if status and status != "off":
            return f"{name}:{status}"
    return _read(root / "sys/devices/system/cpu/cpu0/cpufreq/scaling_driver")


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
    facts = {
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
    assert tuple(facts) == PLATFORM_KEYS
    return facts


def _cpu_list(cpus: set[int]) -> str:
    ranges: list[list[int]] = []
    for cpu in sorted(cpus):
        if ranges and cpu == ranges[-1][1] + 1:
            ranges[-1][1] = cpu
        else:
            ranges.append([cpu, cpu])
    return ",".join(str(low) if low == high else f"{low}-{high}" for low, high in ranges)


def _allowed_cpus(root: Path, pid: int) -> str | None:
    for line in (_read(root / f"proc/{pid}/status") or "").splitlines():
        if line.startswith("Cpus_allowed_list:"):
            return line.partition(":")[2].strip() or None
    return None


def _irq_default_cpus(root: Path) -> str | None:
    try:
        mask = int((_read(root / "proc/irq/default_smp_affinity") or "").replace(",", ""), 16)
    except ValueError:
        return None
    return _cpu_list({cpu for cpu in range(mask.bit_length()) if mask >> cpu & 1})


def cpu_policy_facts(root: Path = Path("/")) -> dict:
    """CPU placement of everything that is not the system under test.

    `isolated_cpus` is empty when no CPU is isolated and `None` when it cannot
    be read. `housekeeping_cpus` is PID 1's affinity, which every service and
    login session inherits; `irq_default_cpus` is the default IRQ affinity.
    """
    try:
        isolated = (root / "sys/devices/system/cpu/isolated").read_text().replace("\x00", "").strip()
    except OSError:
        isolated = None
    facts = {
        "isolated_cpus": isolated,
        "housekeeping_cpus": _allowed_cpus(root, 1),
        "irq_default_cpus": _irq_default_cpus(root),
    }
    assert tuple(facts) == CPU_POLICY_KEYS
    return facts
