#!/usr/bin/env python3

import copy
import json
import subprocess
import sys
import tempfile
import tomllib
from pathlib import Path

ROOT = Path(__file__).resolve().parents[3]
MATRIX = ROOT / "eval/canonical-matrix.json"
VALIDATOR = ROOT / "eval/scripts/validate-canonical.py"


def run_validator(*args: str) -> subprocess.CompletedProcess[str]:
    return subprocess.run(
        [sys.executable, str(VALIDATOR), *args],
        cwd=ROOT,
        capture_output=True,
        text=True,
        check=False,
    )


def write_json(path: Path, value: object) -> None:
    path.write_text(json.dumps(value))


def test_matrix_accepts_frozen_experiments() -> None:
    result = run_validator("matrix", str(MATRIX))
    assert result.returncode == 0, result.stderr
    assert "27 experiments" in result.stdout


def test_rate_sweep_is_frozen_and_diagnostic() -> None:
    matrix = json.loads(MATRIX.read_text())
    sweep = matrix["experiments"]["e-perf-10"]
    profile = ROOT / sweep["loadgen_profile"]

    assert sweep["purpose"] == (
        "Determine sustainable throughput for matched Pipeline A comparators and the MQTT loopback floor."
    )
    assert sweep["systems"] == ["mqtt-loopback", "native", "wafer", "ekuiper"]
    assert sweep["rate_points_msg_s"] == [500, 1000, 2000, 4000, 8000, 16000]
    assert sweep["repetitions"] == 4
    assert sweep["thesis_evidence"] is False
    assert sweep["ordering"] == {
        "method": "seeded rate blocks with four-run rotated system order",
        "default_seed": 1729,
    }
    assert sweep["sustainable_throughput"] == {
        "baseline_rate_msg_s": 1000,
        "p99_multiplier_limit": 2.0,
        "max_loss_percent": 1.0,
        "p99_aggregation": "median across repetitions",
        "loss_aggregation": "sum(lost) / sum(offered)",
        "selection": "last contiguous good rate at or above baseline before first bad rate",
    }
    assert profile.is_file()
    configured = tomllib.loads(profile.read_text())["sweep"]
    assert configured["rate_points_msg_s"] == sweep["rate_points_msg_s"]
    assert configured["repetitions"] == sweep["repetitions"]
    assert configured["baseline_rate_msg_s"] == sweep["sustainable_throughput"]["baseline_rate_msg_s"]
    assert configured["p99_multiplier_limit"] == sweep["sustainable_throughput"]["p99_multiplier_limit"]
    assert configured["max_loss_percent"] == sweep["sustainable_throughput"]["max_loss_percent"]
    assert configured["ordering"] == sweep["ordering"]["method"]
    assert configured["p99_aggregation"] == sweep["sustainable_throughput"]["p99_aggregation"]
    assert configured["loss_aggregation"] == sweep["sustainable_throughput"]["loss_aggregation"]
    assert configured["thesis_evidence"] is False


def test_eiso7_has_matched_control_panic_and_epoch_loop_conditions() -> None:
    matrix = json.loads(MATRIX.read_text())["experiments"]["e-iso-7"]
    assert matrix["conditions"] == ["control", "panic-attack", "epoch-loop-attack"]
    assert "branch-isolation.json" in matrix["required_outputs"]

    paths = (
        "pipeline-control.toml",
        "pipeline.toml",
        "pipeline-epoch-attack.toml",
    )
    configs = [
        tomllib.loads((ROOT / "eval/configs/e-iso-7" / path).read_text())
        for path in paths
    ]
    plugins = [config["nodes"]["branch_b"]["plugin"] for config in configs]
    assert plugins[0].endswith("pass-through/target/wasm32-wasip2/release/wafer_pass_through.wasm")
    assert plugins[1].endswith("panic/target/wasm32-wasip2/release/wafer_attack_panic.wasm")
    assert plugins[2].endswith("infinite-loop/target/wasm32-wasip2/release/wafer_attack_infinite_loop.wasm")
    for config in configs:
        config["nodes"]["branch_b"]["plugin"] = "<fault>"
    assert configs[0] == configs[1] == configs[2]


def test_eperf1_is_labelled_as_target_load_not_saturation_capacity() -> None:
    matrix = json.loads(MATRIX.read_text())
    purpose = matrix["experiments"]["e-perf-1"]["purpose"].lower()
    notebook = json.loads(
        (ROOT / "eval/analysis/notebooks/09-saturation.ipynb").read_text()
    )
    notebook_text = "".join(
        "".join(cell.get("source", [])) for cell in notebook["cells"]
    )

    assert "target-load" in purpose
    assert "not a saturation-capacity measurement" in purpose
    assert "Target-Load Comparison" in notebook_text
    assert "Target-Load Delivery Summary" in notebook_text
    assert "Compares sustainable throughput" not in notebook_text


def test_subcanonical_repetitions_require_diagnostic_label() -> None:
    matrix = json.loads(MATRIX.read_text())
    matrix["experiments"]["e-perf-10"]["thesis_evidence"] = True
    with tempfile.TemporaryDirectory() as tmp:
        path = Path(tmp) / "matrix.json"
        write_json(path, matrix)
        result = run_validator("matrix", str(path))
    assert result.returncode == 1
    assert "e-perf-10 repetitions must be >= 30" in result.stderr


def test_matrix_rejects_missing_experiment() -> None:
    matrix = json.loads(MATRIX.read_text())
    matrix["experiments"].pop("e-swap-6")
    with tempfile.TemporaryDirectory() as tmp:
        path = Path(tmp) / "matrix.json"
        write_json(path, matrix)
        result = run_validator("matrix", str(path))
    assert result.returncode == 1
    assert "missing experiments: e-swap-6" in result.stderr


def test_matrix_rejects_subcanonical_repetitions() -> None:
    matrix = json.loads(MATRIX.read_text())
    matrix["experiments"]["e-perf-4"]["repetitions"] = 29
    with tempfile.TemporaryDirectory() as tmp:
        path = Path(tmp) / "matrix.json"
        write_json(path, matrix)
        result = run_validator("matrix", str(path))
    assert result.returncode == 1
    assert "e-perf-4 repetitions must be >= 30" in result.stderr


def valid_facts() -> dict:
    return {
        "host_tag": "rpi5",
        "arch": "aarch64",
        "hardware_model": "Raspberry Pi 5 Model B Rev 1.0",
        "git_sha": "1" * 40,
        "git_dirty": False,
        "git_tags": ["rpi5-eval-v1"],
        "cpu_governors": ["performance"],
        "isolated_cpus": "1-3",
        "throttled": "0x0",
        "broker_ready": True,
        "ekuiper_ready": True,
        "ekuiper_version": "2.1.0",
    }


def test_preflight_accepts_canonical_facts() -> None:
    with tempfile.TemporaryDirectory() as tmp:
        path = Path(tmp) / "facts.json"
        write_json(path, valid_facts())
        result = run_validator("preflight", str(path), "--require-ekuiper")
    assert result.returncode == 0, result.stderr
    assert "canonical preflight: PASS" in result.stdout


def test_preflight_rejects_each_provenance_and_host_violation() -> None:
    invalid = {
        "dirty source": ("git_dirty", True),
        "untagged source": ("git_tags", []),
        "host tag": ("host_tag", "shakedown-macos"),
        "CPU governor": ("cpu_governors", ["ondemand"]),
        "isolated CPUs": ("isolated_cpus", ""),
        "throttling": ("throttled", "0x50000"),
        "broker": ("broker_ready", False),
        "eKuiper": ("ekuiper_ready", False),
        "eKuiper version": ("ekuiper_version", "2.2.0"),
    }
    for expected, (key, value) in invalid.items():
        facts = valid_facts()
        facts[key] = value
        with tempfile.TemporaryDirectory() as tmp:
            path = Path(tmp) / "facts.json"
            write_json(path, facts)
            result = run_validator("preflight", str(path), "--require-ekuiper")
        assert result.returncode == 1, (key, result.stdout, result.stderr)
        assert expected in result.stderr, (key, result.stderr)


if __name__ == "__main__":
    test_matrix_accepts_frozen_experiments()
    test_eiso7_has_matched_control_panic_and_epoch_loop_conditions()
    test_matrix_rejects_missing_experiment()
    test_matrix_rejects_subcanonical_repetitions()
    test_preflight_accepts_canonical_facts()
    test_preflight_rejects_each_provenance_and_host_violation()
    print("canonical matrix tests: PASS")
