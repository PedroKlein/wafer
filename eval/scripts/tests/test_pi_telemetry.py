#!/usr/bin/env python3

import importlib.util
import os
import subprocess
import sys
import tempfile
import time
from pathlib import Path

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
        time.sleep(0.3)
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
