#!/usr/bin/env python3

import importlib.util
import json
import subprocess
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parents[3]

spec = importlib.util.spec_from_file_location(
    "analyze_instrument_ab", ROOT / "eval/scripts/analyze-instrument-ab.py"
)
assert spec and spec.loader
analyze_instrument_ab = importlib.util.module_from_spec(spec)
spec.loader.exec_module(analyze_instrument_ab)

START = 1_000_000_000_000


def write_leaf(root: Path, arm: str, pair: int, p95_us: float, received: int = 60_000, gaps: int = 0) -> Path:
    leaf = root / arm / f"pair-{pair:02d}"
    leaf.mkdir(parents=True)
    (leaf / "subscriber-metadata.json").write_text(
        json.dumps(
            {
                "latency_p50_ns": int(p95_us * 500),
                "latency_p95_ns": int(p95_us * 1000),
                "latency_p99_ns": int(p95_us * 2000),
                "total_recorded": received,
                "sequence": {"received_unique": received - gaps, "total_gaps": gaps, "total_duplicates": 0},
            }
        )
    )
    (leaf / "measurement-window.json").write_text(
        json.dumps({"started_ns": START, "finished_ns": START + 60_000_000_000})
    )
    if arm == "on":
        (leaf / "host-sidecar.json").write_text(
            json.dumps(
                {
                    "sampler_cpu_seconds": 0.12,
                    "started_unix_epoch_ns": START,
                    "finished_unix_epoch_ns": START + 120_000_000_000,
                }
            )
        )
        (leaf / "pi-telemetry.csv").write_text("timestamp_ns\n")
    return leaf


def test_matching_arms_are_negligible(tmp_path: Path) -> None:
    for pair, (on, off) in enumerate([(800, 790), (805, 810), (795, 800), (802, 798)], start=1):
        write_leaf(tmp_path, "on", pair, on)
        write_leaf(tmp_path, "off", pair, off)
    result = analyze_instrument_ab.analyze(tmp_path)
    assert result["pairs"] == 4
    assert result["thesis_evidence"] is False
    assert abs(result["metrics"]["p95_ns"]["median_relative_difference_percent"]) < 1.0
    assert result["sampler_core_fraction_max"] == 0.001
    assert result["lossless"] is True
    assert result["negligible"] is True


def test_a_slower_sidecar_arm_is_not_negligible(tmp_path: Path) -> None:
    for pair in range(1, 4):
        write_leaf(tmp_path, "on", pair, 900)
        write_leaf(tmp_path, "off", pair, 800)
    result = analyze_instrument_ab.analyze(tmp_path)
    assert result["metrics"]["p95_ns"]["median_relative_difference_percent"] == 12.5
    assert result["negligible"] is False
    completed = subprocess.run(
        [sys.executable, str(ROOT / "eval/scripts/analyze-instrument-ab.py"), str(tmp_path)],
        capture_output=True,
        text=True,
    )
    assert completed.returncode == 1
    assert (tmp_path / "instrument-ab.json").is_file()


def test_off_arm_with_sidecar_output_is_rejected(tmp_path: Path) -> None:
    for pair in range(1, 4):
        write_leaf(tmp_path, "on", pair, 800)
        write_leaf(tmp_path, "off", pair, 800)
    (tmp_path / "off" / "pair-02" / "host-sidecar.json").write_text("{}")
    result = analyze_instrument_ab.analyze(tmp_path)
    assert result["pairs"] == 2
    assert result["rejected_pairs"][0]["pair"] == "pair-02"
    assert result["negligible"] is False


def test_lossy_run_is_not_negligible(tmp_path: Path) -> None:
    for pair in range(1, 3):
        write_leaf(tmp_path, "on", pair, 800, gaps=3 if pair == 1 else 0)
        write_leaf(tmp_path, "off", pair, 800)
    result = analyze_instrument_ab.analyze(tmp_path)
    assert result["lossless"] is False
    assert result["negligible"] is False


def test_runner_dry_run_alternates_arm_order() -> None:
    plan = subprocess.run(
        [str(ROOT / "eval/scripts/run-rpi5-instrument-ab.sh"), "--dry-run", "--pairs", "4"],
        capture_output=True,
        text=True,
        check=True,
    ).stdout
    assert "pair-01: on off" in plan
    assert "pair-02: off on" in plan
    assert "evidence_class: diagnostic" in plan
    assert "pipeline-a-wafer.toml" in plan


def test_run_experiment_skips_sidecars_when_switched_off() -> None:
    script = (ROOT / "eval/scripts/run-experiment.sh").read_text()
    assert '[ "${WAFER_HOST_SIDECARS:-on}" != "off" ] || return 0' in script


def test_messages_missing_at_the_tail_count_as_loss(tmp_path: Path) -> None:
    for pair in range(1, 3):
        write_leaf(tmp_path, "on", pair, 800, received=59_500 if pair == 2 else 60_000)
        write_leaf(tmp_path, "off", pair, 800)
    result = analyze_instrument_ab.analyze(tmp_path)
    assert result["per_pair"][1]["on"]["lost"] == 500
    assert result["lossless"] is False
    assert result["negligible"] is False


def test_an_empty_run_rejects_only_its_pair(tmp_path: Path) -> None:
    for pair in range(1, 4):
        write_leaf(tmp_path, "on", pair, 800)
        write_leaf(tmp_path, "off", pair, 0 if pair == 3 else 800, received=0 if pair == 3 else 60_000)
    result = analyze_instrument_ab.analyze(tmp_path)
    assert result["pairs"] == 2
    assert result["rejected_pairs"][0]["pair"] == "pair-03"
    assert result["negligible"] is False
