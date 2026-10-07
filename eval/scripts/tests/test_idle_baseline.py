#!/usr/bin/env python3

import importlib.util
import json
import subprocess
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parents[3]

spec = importlib.util.spec_from_file_location(
    "summarise_idle_baseline", ROOT / "eval/scripts/summarise-idle-baseline.py"
)
assert spec and spec.loader
summarise_idle_baseline = importlib.util.module_from_spec(spec)
spec.loader.exec_module(summarise_idle_baseline)


def write_sample(
    root: Path, index: int, watts: float, busy_jiffies: int, untracked_jiffies: int = 0
) -> None:
    """``untracked_jiffies`` per second of SUT-core busy time miss the tick-sampled counters."""
    sample = root / f"sample-{index:02d}"
    sample.mkdir(parents=True)
    start = 1_000_000_000_000
    rows = ["timestamp_ns,temperature_millicelsius,cpu_frequency_hz,governor,throttled,rail_proxy_watts"]
    cores = ["timestamp_ns,cpu,user,nice,system,idle,iowait,irq,softirq,steal,frequency_hz"]
    for second in range(0, 11):
        stamp = start + second * 1_000_000_000
        rows.append(f"{stamp},45000,2400000000,performance,0x0,{watts}")
        for cpu in range(4):
            busy = busy_jiffies * second if cpu else 50 * second
            idle = 100 * second - busy - (untracked_jiffies * second if cpu else 0)
            cores.append(f"{stamp},{cpu},{busy},0,0,{idle},0,0,0,0,2400000000")
    (sample / "pi-telemetry.csv").write_text("\n".join(rows) + "\n")
    (sample / "cpu-cores.csv").write_text("\n".join(cores) + "\n")
    (sample / "measurement-window.json").write_text(
        json.dumps({"started_ns": start, "finished_ns": start + 10_000_000_000})
    )
    (sample / "host-sidecar.json").write_text(
        json.dumps({"sut_cpus": "1-3", "sampler_cpu_seconds": 0.05, "clock_ticks_per_second": 100})
    )


def test_summary_reports_median_idle_watts_and_sut_core_idleness(tmp_path: Path) -> None:
    write_sample(tmp_path, 1, 2.0, busy_jiffies=1)
    write_sample(tmp_path, 2, 2.4, busy_jiffies=1)
    write_sample(tmp_path, 3, 2.2, busy_jiffies=1)
    summary = summarise_idle_baseline.summarise(tmp_path)
    assert summary["samples"] == 3
    assert abs(summary["idle_proxy_watts_median"] - 2.2) < 1e-9
    assert summary["thesis_evidence"] is False
    assert summary["is_total_input_power"] is False
    assert abs(summary["sut_core_busy_fraction_max"] - 0.01) < 1e-9
    assert summary["host_idle"] is True
    assert summary["throttled"] is False


def test_sut_core_busy_time_the_tick_missed_still_counts(tmp_path: Path) -> None:
    write_sample(tmp_path, 1, 2.0, busy_jiffies=1, untracked_jiffies=29)
    summary = summarise_idle_baseline.summarise(tmp_path)
    assert abs(summary["sut_core_busy_fraction_max"] - 0.30) < 1e-9
    assert summary["host_idle"] is False


def test_busy_sut_cores_mark_the_host_as_not_idle(tmp_path: Path) -> None:
    write_sample(tmp_path, 1, 2.0, busy_jiffies=30)
    summary = summarise_idle_baseline.summarise(tmp_path)
    assert summary["host_idle"] is False
    assert (
        subprocess.run(
            [sys.executable, str(ROOT / "eval/scripts/summarise-idle-baseline.py"), str(tmp_path)],
            capture_output=True,
        ).returncode
        == 1
    )
    assert (tmp_path / "idle-baseline.json").is_file()


def test_runner_dry_run_prints_the_plan() -> None:
    plan = subprocess.run(
        [str(ROOT / "eval/scripts/run-rpi5-idle-baseline.sh"), "--dry-run", "--samples", "4", "--sample-secs", "30"],
        capture_output=True,
        text=True,
        check=True,
    ).stdout
    assert "experiment: idle-baseline" in plan
    assert "evidence_class: diagnostic" in plan
    assert "samples: 4" in plan
    assert "--pin-cpus 0 --sut-cpus 1-3" in plan


def test_incomplete_sample_is_rejected_and_makes_the_baseline_unusable(tmp_path: Path) -> None:
    write_sample(tmp_path, 1, 2.0, busy_jiffies=1)
    broken = tmp_path / "sample-02"
    broken.mkdir()
    (broken / "telemetry-error.json").write_text("{}")
    summary = summarise_idle_baseline.summarise(tmp_path)
    assert summary["samples"] == 1
    assert summary["rejected_samples"][0]["sample"] == "sample-02"
    assert summary["host_idle"] is True
    assert summary["usable"] is False


def test_power_report_reads_idle_watts_only_from_a_usable_baseline(tmp_path: Path) -> None:
    read_idle_watts = summarise_idle_baseline.power_module.read_idle_watts
    write_sample(tmp_path, 1, 2.0, busy_jiffies=1)
    write_sample(tmp_path, 2, 2.4, busy_jiffies=1)
    path = tmp_path / "idle-baseline.json"
    path.write_text(json.dumps(summarise_idle_baseline.summarise(tmp_path)))
    assert abs(read_idle_watts(path) - 2.2) < 1e-9

    busy = json.loads(path.read_text()) | {"usable": False}
    path.write_text(json.dumps(busy))
    try:
        read_idle_watts(path)
    except ValueError as error:
        assert "idle, unthrottled" in str(error)
    else:
        raise AssertionError("an unusable baseline must be refused")
