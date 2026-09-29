#!/usr/bin/env python3

import importlib.util
import json
import os
import subprocess
import sys
import tempfile
import time
from pathlib import Path

import pytest

ROOT = Path(__file__).resolve().parents[3]
sys.path.insert(0, str(ROOT / "eval/scripts/lib"))
sys.path.insert(0, str(ROOT / "eval/analysis/src"))

from pi_telemetry import BOUNDARY, parse_pmic  # noqa: E402

power_spec = importlib.util.spec_from_file_location(
    "wafer_power", ROOT / "eval/analysis/src/wafer_analysis/power.py"
)
assert power_spec and power_spec.loader
power_module = importlib.util.module_from_spec(power_spec)
power_spec.loader.exec_module(power_module)
summarize_power = power_module.summarize_power

PMIC = """
   3V3_SYS_A current(1)=0.12687090A
   1V8_SYS_A current(2)=0.20299340A
  VDD_CORE_A current(7)=1.91102000A
   3V3_SYS_V volt(9)=3.32952100V
   1V8_SYS_V volt(10)=1.80317300V
  VDD_CORE_V volt(15)=0.87355220V
     EXT5V_V volt(24)=5.09066000V
"""


def test_pmic_parser_pairs_named_internal_rails_and_excludes_unpaired_input() -> None:
    rails = parse_pmic(PMIC)
    names = {rail["rail"] for rail in rails}
    assert names == {"1V8_SYS", "3V3_SYS", "VDD_CORE"}
    assert "EXT5V" not in names
    core = next(rail for rail in rails if rail["rail"] == "VDD_CORE")
    assert abs(float(core["power_w"]) - 1.669375725244) < 0.000001
    assert BOUNDARY["is_total_input_power"] is False
    assert "USB current" in BOUNDARY["excludes"]


def test_sampler_flushes_on_termination_and_marks_failures_separately() -> None:
    with tempfile.TemporaryDirectory() as tmp:
        root = Path(tmp)
        binary = root / "bin"
        binary.mkdir()
        fake = binary / "vcgencmd"
        fake.write_text(
            "#!/bin/sh\n"
            "case \"$1\" in\n"
            "  pmic_read_adc) printf '3V3_SYS_A current(1)=0.10000000A\\n3V3_SYS_V volt(9)=3.30000000V\\n' ;;\n"
            "  get_throttled) echo throttled=0x0 ;;\n"
            "esac\n"
        )
        fake.chmod(0o755)
        output = root / "output"
        environment = os.environ.copy()
        environment["PATH"] = f"{binary}:{environment['PATH']}"
        process = subprocess.Popen(
            [sys.executable, str(ROOT / "eval/scripts/lib/pi_telemetry.py"), str(output), "0.05"],
            env=environment,
        )
        deadline = time.monotonic() + 5
        telemetry = output / "pi-telemetry.csv"
        while time.monotonic() < deadline:
            if process.poll() is not None:
                break
            if telemetry.is_file() and len(telemetry.read_text().splitlines()) >= 2:
                break
            time.sleep(0.02)
        assert process.poll() is None, "telemetry sampler exited before becoming ready"
        assert telemetry.is_file(), "telemetry sampler did not become ready"
        process.terminate()
        assert process.wait(timeout=5) == 0
        assert (output / "power-boundary.json").is_file()
        assert len((output / "pi-telemetry.csv").read_text().splitlines()) >= 2
        assert len((output / "pmic-rails.csv").read_text().splitlines()) >= 2
        assert not (output / "telemetry-error.json").exists()


def test_power_summary_integrates_proxy_energy_and_idle_adjustment() -> None:
    samples = [
        {
            "timestamp_ns": 0,
            "temperature_millicelsius": 50_000,
            "cpu_frequency_hz": 2_400_000_000,
            "governor": "performance",
            "throttled": "0x0",
            "rail_proxy_watts": 4.0,
        },
        {
            "timestamp_ns": 1_000_000_000,
            "temperature_millicelsius": 52_000,
            "cpu_frequency_hz": 2_400_000_000,
            "governor": "performance",
            "throttled": "0x0",
            "rail_proxy_watts": 6.0,
        },
        {
            "timestamp_ns": 2_000_000_000,
            "temperature_millicelsius": 53_000,
            "cpu_frequency_hz": 2_400_000_000,
            "governor": "performance",
            "throttled": "0x0",
            "rail_proxy_watts": 4.0,
        },
    ]
    summary = summarize_power(samples, idle_watts=2.0, messages=100)
    assert summary["proxy_energy_j"] == 10.0
    assert summary["mean_proxy_watts"] == 5.0
    assert summary["idle_adjusted_proxy_energy_j"] == 6.0
    assert summary["proxy_energy_per_message_j"] == 0.06
    assert summary["is_total_input_power"] is False


if __name__ == "__main__":
    test_pmic_parser_pairs_named_internal_rails_and_excludes_unpaired_input()
    test_sampler_flushes_on_termination_and_marks_failures_separately()
    test_power_summary_integrates_proxy_energy_and_idle_adjustment()
    print("Pi telemetry tests: PASS")


def _write(path: Path, text: str) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(text)


def _cpufreq(root: Path, cpu: int, current_khz: int, max_khz: int = 2_000_000) -> None:
    cpufreq = root / f"sys/devices/system/cpu/cpu{cpu}/cpufreq"
    _write(cpufreq / "scaling_cur_freq", f"{current_khz}\n")
    _write(cpufreq / "scaling_max_freq", f"{max_khz}\n")
    _write(cpufreq / "scaling_governor", "performance\n")


def _jetson_sysroot(root: Path) -> Path:
    _write(root / "etc/nv_tegra_release", "# R36 (release), REVISION: 3.0\n")
    hwmon = root / "sys/class/hwmon/hwmon1"
    _write(hwmon / "name", "ina3221\n")
    rails = {"VDD_IN": (5000, 1200), "VDD_CPU_GPU_CV": (1000, 800), "VDD_SOC": (900, 400)}
    for channel, (label, (mv, ma)) in enumerate(rails.items(), start=1):
        _write(hwmon / f"in{channel}_label", f"{label}\n")
        _write(hwmon / f"in{channel}_input", f"{mv}\n")
        _write(hwmon / f"curr{channel}_input", f"{ma}\n")
    _write(root / "sys/class/thermal/thermal_zone0/type", "gpu-thermal\n")
    _write(root / "sys/class/thermal/thermal_zone0/temp", "41000\n")
    _write(root / "sys/class/thermal/thermal_zone1/type", "cpu-thermal\n")
    _write(root / "sys/class/thermal/thermal_zone1/temp", "47500\n")
    for cpu in range(4):
        _cpufreq(root, cpu, 2_000_000)
    return root


def _x86_sysroot(root: Path) -> Path:
    package = root / "sys/class/powercap/intel-rapl:0"
    _write(package / "name", "package-0\n")
    _write(package / "energy_uj", "1000000\n")
    _write(package / "max_energy_range_uj", "262143328850\n")
    _write(root / "sys/class/powercap/intel-rapl:0:0/name", "core\n")
    _write(root / "sys/class/powercap/intel-rapl:0:0/energy_uj", "1\n")
    _write(root / "sys/class/thermal/thermal_zone0/type", "acpitz\n")
    _write(root / "sys/class/thermal/thermal_zone0/temp", "27800\n")
    _write(root / "sys/class/thermal/thermal_zone1/type", "x86_pkg_temp\n")
    _write(root / "sys/class/thermal/thermal_zone1/temp", "61000\n")
    for cpu in range(4):
        _cpufreq(root, cpu, 3_400_000, 3_400_000)
        _write(root / f"sys/devices/system/cpu/cpu{cpu}/thermal_throttle/core_throttle_count", "0\n")
    return root


def test_jetson_backend_reads_ina3221_rails_and_the_cpu_thermal_zone(tmp_path: Path) -> None:
    from pi_telemetry import JETSON_BOUNDARY, make_backend, sample

    backend = make_backend("auto", _jetson_sysroot(tmp_path))
    assert backend.name == "jetson" and backend.boundary is JETSON_BOUNDARY
    summary, rails = sample(backend)
    assert [rail["rail"] for rail in rails] == ["VDD_IN", "VDD_CPU_GPU_CV", "VDD_SOC"]
    assert abs(float(rails[0]["power_w"]) - 6.0) < 1e-9
    assert abs(float(summary["rail_proxy_watts"]) - 7.16) < 1e-9
    assert summary["temperature_millicelsius"] == 47500
    assert summary["throttled"] == "0x0"
    assert summary["cpu_frequency_hz"] == 2_000_000_000


def test_jetson_backend_flags_a_core_running_below_its_pinned_clock(tmp_path: Path) -> None:
    from pi_telemetry import make_backend, sample

    root = _jetson_sysroot(tmp_path)
    _cpufreq(root, 2, 1_500_000)
    summary, _ = sample(make_backend("jetson", root))
    assert summary["throttled"] == "cpu2-below-pinned-clock"


def test_x86_backend_derives_package_power_from_rapl_energy_deltas(tmp_path: Path) -> None:
    from pi_telemetry import X86_RAPL_BOUNDARY, make_backend

    root = _x86_sysroot(tmp_path)
    backend = make_backend("x86", root)
    assert backend.boundary is X86_RAPL_BOUNDARY
    assert backend.rails(1_000_000_000) == []
    _write(root / "sys/class/powercap/intel-rapl:0/energy_uj", "16000000\n")
    rails = backend.rails(2_000_000_000)
    assert [rail["rail"] for rail in rails] == ["package-0"]
    assert abs(float(rails[0]["power_w"]) - 15.0) < 1e-9
    _write(root / "sys/class/powercap/intel-rapl:0/energy_uj", "5000000\n")
    wrapped = backend.rails(3_000_000_000)
    assert abs(float(wrapped[0]["power_w"]) - (262143328850 - 11000000) / 1e6) < 1e-6


def test_x86_backend_reports_thermal_throttle_counter_increments(tmp_path: Path) -> None:
    from pi_telemetry import make_backend, sample

    root = _x86_sysroot(tmp_path)
    backend = make_backend("x86", root)
    summary, _ = sample(backend)
    assert summary["throttled"] == "0x0"
    assert summary["temperature_millicelsius"] == 61000
    _write(root / "sys/devices/system/cpu/cpu1/thermal_throttle/core_throttle_count", "3\n")
    summary, _ = sample(backend)
    assert summary["throttled"] == "cpu1-thermal-throttle"
    summary, _ = sample(backend)
    assert summary["throttled"] == "0x0"


def test_x86_backend_without_rapl_declares_power_unavailable_and_uses_hwmon_temperature(
    tmp_path: Path,
) -> None:
    from pi_telemetry import X86_NO_POWER_BOUNDARY, make_backend, sample

    _write(tmp_path / "sys/class/thermal/thermal_zone0/type", "acpitz\n")
    _write(tmp_path / "sys/class/thermal/thermal_zone0/temp", "27800\n")
    _write(tmp_path / "sys/class/hwmon/hwmon2/name", "k10temp\n")
    _write(tmp_path / "sys/class/hwmon/hwmon2/temp1_input", "55125\n")
    unreadable = tmp_path / "sys/class/powercap/intel-rapl:0"
    _write(unreadable / "name", "package-0\n")
    (unreadable / "energy_uj").mkdir()
    backend = make_backend("x86", tmp_path)
    assert backend.boundary is X86_NO_POWER_BOUNDARY
    summary, rails = sample(backend)
    assert rails == [] and summary["rail_proxy_watts"] == 0
    assert summary["temperature_millicelsius"] == 55125


def test_backend_detection_prefers_vcgencmd_then_l4t_then_x86(tmp_path: Path, monkeypatch) -> None:
    import pi_telemetry

    monkeypatch.setattr(pi_telemetry.shutil, "which", lambda name: None)
    assert pi_telemetry.detect_backend_name(_jetson_sysroot(tmp_path / "jetson")) == "jetson"
    assert pi_telemetry.detect_backend_name(tmp_path / "empty", machine="x86_64") == "x86"
    with pytest.raises(OSError):
        pi_telemetry.detect_backend_name(tmp_path / "empty", machine="riscv64")
    monkeypatch.setattr(pi_telemetry.shutil, "which", lambda name: "/usr/bin/vcgencmd")
    assert pi_telemetry.detect_backend_name(_jetson_sysroot(tmp_path / "jetson")) == "pi"


def test_sampler_writes_the_same_files_on_a_jetson_sysroot(tmp_path: Path) -> None:
    root = _jetson_sysroot(tmp_path / "root")
    output = tmp_path / "output"
    process = subprocess.Popen(
        [
            sys.executable,
            str(ROOT / "eval/scripts/lib/pi_telemetry.py"),
            str(output),
            "0.05",
            "--backend",
            "jetson",
            "--sysroot",
            str(root),
        ]
    )
    deadline = time.monotonic() + 5
    telemetry = output / "pi-telemetry.csv"
    while time.monotonic() < deadline and process.poll() is None:
        if telemetry.is_file() and len(telemetry.read_text().splitlines()) >= 2:
            break
        time.sleep(0.02)
    process.terminate()
    assert process.wait(timeout=5) == 0
    boundary = json.loads((output / "power-boundary.json").read_text())
    assert boundary["backend"] == "jetson"
    assert boundary["measurement"] == "jetson-ina3221-rail-proxy"
    assert len((output / "pmic-rails.csv").read_text().splitlines()) >= 4
    assert not (output / "telemetry-error.json").exists()


def test_sampler_records_a_missing_backend_instead_of_dying_silently(tmp_path: Path, monkeypatch) -> None:
    import pi_telemetry

    monkeypatch.setattr(pi_telemetry.shutil, "which", lambda name: None)
    monkeypatch.setattr(pi_telemetry.platform, "machine", lambda: "riscv64")
    assert pi_telemetry.run(tmp_path / "leaf", 0.05, "auto", tmp_path / "empty") == 1
    error = json.loads((tmp_path / "leaf" / "telemetry-error.json").read_text())
    assert "no telemetry backend" in error["error"]
