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
    assert "schedule_records=2105" in result.stdout
    assert "measured_leaves=1893" in result.stdout


def test_focused_pilot_selection_is_exact_and_diagnostic() -> None:
    matrix = json.loads(MATRIX.read_text())
    focused = matrix["focused_pilot"]
    expected = {
        "e-perf-10": {
            f"{system}/rate-{rate:05d}": [1, 2, 3, 4]
            for system in ("mqtt-loopback", "native", "wafer", "ekuiper")
            for rate in (500, 1000, 2000, 4000, 8000, 16000)
        },
        "e-perf-9": {
            f"{tier}-{cache}": [1, 2, 3]
            for tier in ("small", "medium", "large")
            for cache in ("cold", "warm")
        },
        "e-backpressure": {"saturated-slow-consumer": [1, 2, 3]},
        "e-iso-4": {"infinite-loop": [1, 2, 3]},
        "e-iso-7": {
            condition: [1, 2, 3]
            for condition in ("control", "panic-attack", "epoch-loop-attack")
        },
        "e-swap-1": {"steady": [1]},
        "e-swap-2": {"steady": [1]},
        "e-swap-3": {"wafer-hotswap": [1, 2, 3]},
        "e-swap-4": {"burst-2x": [1]},
        "e-swap-5": {"process-trap-rollback": [1]},
        "e-swap-6": {"steady": [1]},
    }

    assert focused["thesis_evidence"] is False
    assert {key: value["condition_runs"] for key, value in focused["experiments"].items()} == expected
    assert focused["decisions"]["ekuiper_operator_concurrency"] == {
        "value": 1,
        "comparison": "default-system",
    }
    assert focused["decisions"]["memory_retention"] == {
        "status": "fixed",
        "fix_commit": "042575458dd809c5ed410bbde1b087a924cc81c2",
        "diagnostic_runs": 2,
        "max_observed_slope_bytes_per_message": 0.0,
        "canonical_measurement_secs_adequate": True,
    }

    inherited = {
        "sample_unit",
        "repetitions",
        "warmup_secs",
        "measurement_secs",
        "measurement_boundary",
        "required_outputs",
        "analysis",
        "thesis_evidence",
    }
    for definition in focused["experiments"].values():
        assert inherited <= definition.keys()
        assert definition["thesis_evidence"] is False
        if definition["sample_unit"] == "event":
            assert definition["events_per_run"] == 50


def test_focused_pilot_rejects_each_missing_condition_contract_field() -> None:
    required = {
        "sample_unit",
        "repetitions",
        "warmup_secs",
        "measurement_secs",
        "measurement_boundary",
        "condition_runs",
        "required_outputs",
        "analysis",
        "thesis_evidence",
    }
    for field in required:
        matrix = json.loads(MATRIX.read_text())
        del matrix["focused_pilot"]["experiments"]["e-iso-7"][field]
        with tempfile.TemporaryDirectory() as tmp:
            path = Path(tmp) / "matrix.json"
            write_json(path, matrix)
            result = run_validator("matrix", str(path))
        assert result.returncode == 1, (field, result.stdout, result.stderr)
        assert f"focused_pilot e-iso-7 missing fields: {field}" in result.stderr


def test_final_campaign_policy_is_frozen_in_matrix() -> None:
    matrix = json.loads(MATRIX.read_text())
    campaign = matrix["final_campaign"]
    sweep = matrix["experiments"]["e-perf-10"]

    assert campaign["status"] == "frozen-before-execution"
    assert campaign["seed"] == 1729
    assert campaign["thesis_evidence"] is True
    assert campaign["expected_schedule_records"] == 2105
    assert campaign["expected_measured_leaves"] == 1893
    assert campaign["capacity_grid"] == {
        "source_batch_id": "capacity-scout-v3-20260904T045000Z",
        "source_summary_sha256": "04531979da50f882eee2e0d04ab6f25d4002af21519a4c8b5ada6c88c13452b5",
        "candidate_sha256": "5f2231ef541c36c4fef3655ed25239ca028fb7ed7cd1387a3dd6644818cbfc3f",
        "common_rate_points_msg_s": [1000, 4000, 8000, 15000, 16000],
    }
    assert campaign["canonical_metering"] == {
        "policy": "explicit-fuel-and-epoch",
        "fuel": {"transform": 10_000_000, "filter": 500_000, "router": 500_000},
        "epoch_deadline": 100,
        "epoch_tick_ms": 10,
    }
    assert campaign["ekuiper_operator_concurrency"] == 1
    assert len(campaign["wafer_config_catalog"]) == 53
    assert all(set(entry) == {"experiment", "condition", "config"} for entry in campaign["wafer_config_catalog"])
    assert matrix["experiments"]["e-iso-4"]["metering_exceptions"]["infinite-loop"]["epoch_deadline"] == 1
    assert matrix["experiments"]["e-iso-5"]["metering_exceptions"]["memory-exhaust"]["fuel"] is None
    assert set(matrix["experiments"]["e-iso-7"]["metering_exceptions"]) == {"epoch-loop-attack"}
    assert sweep["systems"] == ["mqtt-loopback", "native", "wafer", "ekuiper"]
    assert sweep["rate_points_msg_s"] == [1000, 4000, 8000, 15000, 16000]
    assert sweep["repetitions"] == 30
    assert sweep["sample_unit"] == "run"
    assert sweep["thesis_evidence"] is True
    assert sweep["ordering"] == {
        "method": "seeded rate blocks with thirty-run balanced system order",
        "default_seed": 1729,
    }
    assert sweep["capacity_envelope"]["support_path_censoring"] == "mqtt-loopback"
    assert sweep["capacity_envelope"]["competitive_ratio_threshold"] == 0.70
    assert {"publisher-summary.json", "capacity-run.json", "subscriber-metadata.json"} <= set(
        sweep["required_outputs"]
    )

    assert matrix["experiments"]["e-perf-9"]["cache_scope"] == "linux-filesystem-page-cache"
    assert matrix["experiments"]["e-perf-5"]["incomplete_until"] == "matching x86 Linux batch"
    assert matrix["experiments"]["e-swap-3"]["conditions"] == [
        "wafer-hotswap",
        "wafer-restart",
        "ekuiper-restart",
    ]
    burst = matrix["experiments"]["e-swap-4"]
    assert burst["sample_unit"] == "run"
    assert burst["repetitions"] == 30
    assert "events_per_run" not in burst
    assert burst["burst_profile"] == {
        "before_rate_msg_s": 1000,
        "burst_rate_msg_s": 2000,
        "after_rate_msg_s": 1000,
        "burst_start_secs": 55,
        "swap_secs": 60,
        "burst_end_secs": 65,
        "swaps_per_run": 1,
    }
    assert all(
        definition["thesis_evidence"] is True
        for definition in matrix["experiments"].values()
    )


def test_final_matrix_rejects_capacity_or_burst_drift() -> None:
    mutations = (
        ("e-perf-10 repetitions", lambda value: value["experiments"]["e-perf-10"].update(repetitions=29)),
        ("e-swap-4 repetitions", lambda value: value["experiments"]["e-swap-4"].update(repetitions=29)),
        ("e-perf-10 rate grid", lambda value: value["experiments"]["e-perf-10"].update(rate_points_msg_s=[1000, 4000])),
        ("e-swap-4 sample_unit", lambda value: value["experiments"]["e-swap-4"].update(sample_unit="")),
    )
    for expected, mutate in mutations:
        matrix = json.loads(MATRIX.read_text())
        mutate(matrix)
        with tempfile.TemporaryDirectory() as tmp:
            path = Path(tmp) / "matrix.json"
            write_json(path, matrix)
            result = run_validator("matrix", str(path))
        assert result.returncode == 1, expected
        assert expected in result.stderr


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
    for config in configs:
        assert {"source_a", "source_b"} <= config["nodes"].keys()
        assert "source" not in config["nodes"]
        assert {("source_a", "branch_a"), ("source_b", "branch_b")} <= {
            (edge["from"], edge["to"]) for edge in config["edges"]
        }
        for source in ("source_a", "source_b"):
            assert config["nodes"][source]["kind"] == "bench-source"
            assert config["nodes"][source]["warmup_messages"] == 30_000
            assert config["nodes"][source]["total_messages"] == 90_000

    plugins = [config["nodes"]["branch_b"]["plugin"] for config in configs]
    assert plugins[0].endswith("pass-through/target/wasm32-wasip2/release/wafer_pass_through.wasm")
    assert plugins[1].endswith("panic/target/wasm32-wasip2/release/wafer_attack_panic.wasm")
    assert plugins[2].endswith("infinite-loop/target/wasm32-wasip2/release/wafer_attack_infinite_loop.wasm")
    for config in configs:
        config["nodes"]["branch_b"]["plugin"] = "<fault>"
        config.pop("engine")
    assert configs[0] == configs[1] == configs[2]


def test_backpressure_freezes_internal_queue_pressure_contract() -> None:
    experiment = json.loads(MATRIX.read_text())["experiments"]["e-backpressure"]
    config = tomllib.loads((ROOT / experiment["config"]).read_text())

    assert experiment["conditions"] == ["saturated-slow-consumer"]
    assert experiment["queue_occupancy_threshold"] == 0.8
    assert experiment["queue_recovery_threshold"] == 0.1
    assert experiment["rss_limit_bytes"] == 268_435_456
    assert {"queue-depth.csv", "backpressure.json", "memory.csv", "sequence.csv"} <= set(
        experiment["required_outputs"]
    )
    assert config["nodes"]["source"]["kind"] == "bench-source"
    assert config["nodes"]["source"]["rate"] == 1000.0
    assert config["nodes"]["slow"]["config"]["delay_ms"] == 5
    assert config["edges"][0]["capacity"] == 64
    assert config["edges"][0]["overflow"] == "slow"


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
    assert "target-load comparison" in notebook_text.lower()
    assert "target-load delivery summary" in notebook_text.lower()
    assert "Compares sustainable throughput" not in notebook_text


def test_final_capacity_repetitions_cannot_drop_below_30() -> None:
    matrix = json.loads(MATRIX.read_text())
    matrix["experiments"]["e-perf-10"]["repetitions"] = 29
    with tempfile.TemporaryDirectory() as tmp:
        path = Path(tmp) / "matrix.json"
        write_json(path, matrix)
        result = run_validator("matrix", str(path))
    assert result.returncode == 1
    assert "e-perf-10 repetitions must be exactly 30" in result.stderr


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
