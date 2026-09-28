#!/usr/bin/env python3
import json
import subprocess
import sys
import tempfile
from pathlib import Path

ROOT = Path(__file__).resolve().parents[3]
sys.path.insert(0, str(ROOT / "eval/scripts/lib"))

from host_facts import PLATFORM_KEYS, platform_facts  # noqa: E402
import canonical_runner  # noqa: E402

KEYS = {
    "hardware_model", "cpu_model", "physical_cores", "online_cpus", "smt",
    "turbo", "cpufreq_driver", "os_release", "glibc_version", "power_mode",
}


def write(root: Path, relative: str, text: str) -> None:
    path = root / relative
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(text)


def fake_runner(outputs: dict[str, str]):
    def run(command: list[str]) -> str | None:
        return outputs.get(command[0])
    return run


def test_pi_reads_device_tree_and_leaves_absent_facts_null() -> None:
    with tempfile.TemporaryDirectory() as tmp:
        root = Path(tmp)
        write(root, "proc/device-tree/model", "Raspberry Pi 5 Model B Rev 1.0\x00")
        write(root, "proc/cpuinfo", "processor\t: 0\nBogoMIPS\t: 108.00\n\nprocessor\t: 1\n\n")
        write(root, "sys/devices/system/cpu/online", "0-3\n")
        write(root, "etc/os-release", 'NAME="Debian GNU/Linux"\nPRETTY_NAME="Debian GNU/Linux 13 (trixie)"\n')
        facts = platform_facts(root, fake_runner({"getconf": "glibc 2.41"}))

    assert set(facts) == KEYS
    assert facts["hardware_model"] == "Raspberry Pi 5 Model B Rev 1.0"
    assert facts["cpu_model"] is None
    assert facts["physical_cores"] == 4
    assert facts["online_cpus"] == "0-3"
    assert facts["smt"] is None
    assert facts["turbo"] is None
    assert facts["cpufreq_driver"] is None
    assert facts["os_release"] == "Debian GNU/Linux 13 (trixie)"
    assert facts["glibc_version"] == "2.41"
    assert facts["power_mode"] is None


def test_x86_falls_back_to_dmi_and_reports_smt_and_turbo() -> None:
    cpuinfo = "".join(
        f"processor\t: {cpu}\nmodel name\t: AMD Ryzen 7 5800X 8-Core Processor\n"
        f"physical id\t: 0\ncore id\t: {cpu // 2}\n\n"
        for cpu in range(4)
    )
    with tempfile.TemporaryDirectory() as tmp:
        root = Path(tmp)
        write(root, "proc/cpuinfo", cpuinfo)
        write(root, "sys/class/dmi/id/sys_vendor", "ASUS\n")
        write(root, "sys/class/dmi/id/product_name", "PRIME B550M\n")
        write(root, "sys/devices/system/cpu/online", "0-3\n")
        write(root, "sys/devices/system/cpu/smt/control", "off\n")
        write(root, "sys/devices/system/cpu/cpufreq/boost", "0\n")
        write(root, "sys/devices/system/cpu/cpu0/cpufreq/scaling_driver", "acpi-cpufreq\n")
        write(root, "sys/devices/system/cpu/amd_pstate/status", "passive\n")
        facts = platform_facts(root, fake_runner({"getconf": "glibc 2.35"}))

    assert facts["hardware_model"] == "ASUS PRIME B550M"
    assert facts["cpu_model"] == "AMD Ryzen 7 5800X 8-Core Processor"
    assert facts["physical_cores"] == 2
    assert facts["smt"] == "off"
    assert facts["turbo"] == "off"
    assert facts["cpufreq_driver"] == "amd_pstate:passive"
    assert facts["power_mode"] is None


def test_intel_no_turbo_and_jetson_power_mode() -> None:
    with tempfile.TemporaryDirectory() as tmp:
        root = Path(tmp)
        write(root, "sys/devices/system/cpu/intel_pstate/no_turbo", "1\n")
        write(root, "sys/devices/system/cpu/intel_pstate/status", "active\n")
        assert platform_facts(root, fake_runner({}))["turbo"] == "off"
        assert platform_facts(root, fake_runner({}))["cpufreq_driver"] == "intel_pstate:active"

    with tempfile.TemporaryDirectory() as tmp:
        root = Path(tmp)
        write(root, "proc/device-tree/model", "NVIDIA Jetson Orin Nano Developer Kit\x00")
        write(root, "sys/devices/system/cpu/online", "0-3\n")
        facts = platform_facts(
            root, fake_runner({"nvpmodel": "NV Power Mode: 25W\n1\n"})
        )
    assert facts["hardware_model"] == "NVIDIA Jetson Orin Nano Developer Kit"
    assert facts["physical_cores"] == 4
    assert facts["power_mode"] == "25W"


def test_cpu_model_uses_first_compatible_entry_when_cpuinfo_names_none() -> None:
    with tempfile.TemporaryDirectory() as tmp:
        root = Path(tmp)
        (root / "proc/device-tree").mkdir(parents=True)
        (root / "proc/device-tree/compatible").write_bytes(b"raspberrypi,5-model-b\x00brcm,bcm2712\x00")
        write(root, "proc/cpuinfo", "processor\t: 0\n\n")
        facts = platform_facts(root, fake_runner({}))
    assert facts["cpu_model"] == "raspberrypi,5-model-b"


def test_disabled_pstate_reports_the_real_scaling_driver() -> None:
    with tempfile.TemporaryDirectory() as tmp:
        root = Path(tmp)
        write(root, "sys/devices/system/cpu/intel_pstate/status", "off\n")
        write(root, "sys/devices/system/cpu/cpu0/cpufreq/scaling_driver", "acpi-cpufreq\n")
        facts = platform_facts(root, fake_runner({}))
    assert facts["cpufreq_driver"] == "acpi-cpufreq"


def test_static_measurement_metadata_carries_platform_facts() -> None:
    facts = {
        "git_sha": "a" * 40, "git_dirty": False, "git_tags": ["v1"], "arch": "aarch64",
        "isolated_cpus": "1-3", "cpu_governors": ["performance"], "throttled": "0x0",
        "hardware_model": "Raspberry Pi 5 Model B Rev 1.0", "cpu_model": None,
        "physical_cores": 4, "online_cpus": "0-3", "smt": None, "turbo": None,
        "cpufreq_driver": "cpufreq-dt", "os_release": "Debian GNU/Linux 13 (trixie)",
        "glibc_version": "2.41", "power_mode": None,
    }
    metadata = canonical_runner.static_host_metadata(facts)
    assert set(PLATFORM_KEYS) <= set(metadata)
    assert metadata["hardware_model"] == facts["hardware_model"]
    assert metadata["glibc_version"] == "2.41"
    assert metadata["power_mode"] is None
    assert metadata["git_tags"] == ["v1"]


def test_empty_root_yields_all_keys_as_null() -> None:
    with tempfile.TemporaryDirectory() as tmp:
        facts = platform_facts(Path(tmp), fake_runner({}))
    assert set(facts) == KEYS
    assert all(value is None for value in facts.values())


def test_validate_canonical_host_writes_platform_facts() -> None:
    with tempfile.TemporaryDirectory() as tmp:
        out = Path(tmp) / "host-facts.json"
        subprocess.run(
            [sys.executable, str(ROOT / "eval/scripts/validate-canonical.py"), "host",
             "--root", str(ROOT), "--output", str(out)],
            check=False, capture_output=True, text=True,
        )
        facts = json.loads(out.read_text())
    assert KEYS <= set(facts)
    assert facts["hardware_model"] not in ("", None)
    assert facts["host_tag"] == "rpi5"
