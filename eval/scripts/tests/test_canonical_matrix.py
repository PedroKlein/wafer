#!/usr/bin/env python3

import copy
import json
import subprocess
import sys
import tempfile
import tomllib
from pathlib import Path

import pytest

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


def test_final_matrix_rejects_swap_session_drift() -> None:
    mutations = (
        ("e-swap-1 repetitions must be exactly 10", lambda value: value["experiments"]["e-swap-1"].update(repetitions=1)),
        ("e-swap-5 repetitions must be exactly 10", lambda value: value["experiments"]["e-swap-5"].update(repetitions=30)),
        ("e-swap-5 events_per_run must be exactly 50", lambda value: value["experiments"]["e-swap-5"].update(events_per_run=10)),
        ("e-swap-1 has invalid sample_unit 'event'", lambda value: value["experiments"]["e-swap-1"].update(sample_unit="event")),
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


def test_payload_sizes_pair_a_wafer_arm_with_a_matching_native_arm() -> None:
    matrix = json.loads(MATRIX.read_text())
    payload = matrix["experiments"]["e-perf-4"]
    sizes = ["120b", "1kb", "10kb", "100kb"]
    assert payload["conditions"] == [*sizes, *(f"native-{size}" for size in sizes)]
    assert {"service.hdr", "service-percentiles.json"} <= set(payload["required_outputs"])

    def mutated(change) -> subprocess.CompletedProcess[str]:
        candidate = copy.deepcopy(matrix)
        change(candidate)
        with tempfile.TemporaryDirectory() as tmp:
            path = Path(tmp) / "matrix.json"
            write_json(path, candidate)
            return run_validator("matrix", str(path))

    def drop_native_arm(candidate: dict) -> None:
        candidate["experiments"]["e-perf-4"]["conditions"].remove("native-10kb")

    def drop_service_time(candidate: dict) -> None:
        candidate["experiments"]["e-perf-4"]["required_outputs"].remove("service.hdr")

    def mismatch_payload(candidate: dict) -> None:
        for entry in candidate["final_campaign"]["wafer_config_catalog"]:
            if (entry["experiment"], entry["condition"]) == ("e-perf-4", "120b"):
                entry["config"] = "eval/configs/canonical/e-perf-4-1kb.toml"

    expected = {
        drop_native_arm: "e-perf-4 must pair every WAFER payload size with a native arm",
        drop_service_time: "e-perf-4 required outputs lack the service-time histogram and summary",
        mismatch_payload: "e-perf-4 120b native arm differs from the WAFER arm beyond the transform",
    }
    for change, message in expected.items():
        result = mutated(change)
        assert result.returncode == 1
        assert message in result.stderr


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


def verdict_row(matrix: dict, criterion: str) -> dict:
    return next(
        row for row in matrix["verdict_rules"]["thresholds"] if row["criterion"] == criterion
    )


def test_runner_validation_band_matches_the_declared_thresholds(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    monkeypatch.syspath_prepend(str(ROOT / "eval/scripts/lib"))
    import canonical_runner

    matrix = json.loads(MATRIX.read_text())
    assert (canonical_runner.E_VAL_1_MIN_P99_NS, canonical_runner.E_VAL_1_MAX_P99_NS) == (
        verdict_row(matrix, "e-val-1-p99-low")["value"],
        verdict_row(matrix, "e-val-1-p99-high")["value"],
    )


def test_runner_placebo_instant_matches_the_declared_offset(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    monkeypatch.syspath_prepend(str(ROOT / "eval/scripts/lib"))
    import canonical_runner

    alignment = json.loads(MATRIX.read_text())["experiments"]["e-swap-3"]["event_alignment"]
    assert alignment["placebo_offset_secs"] * 1_000_000_000 == canonical_runner.SWAP3_PLACEBO_OFFSET_NS
    low, high = alignment["event_window_secs"]
    baseline_low, baseline_high = alignment["baseline_window_secs"]
    assert baseline_low <= alignment["placebo_offset_secs"] + low
    assert alignment["placebo_offset_secs"] + high <= baseline_high


@pytest.mark.parametrize(
    ("mutate", "message"),
    [
        (lambda matrix: matrix.pop("verdict_rules"), "verdict_rules must be an object"),
        (
            lambda matrix: matrix["verdict_rules"].update(thresholds=[]),
            "verdict_rules thresholds must be a non-empty list",
        ),
        (
            lambda matrix: verdict_row(matrix, "e-swap-3-dip").pop("origin"),
            "verdict threshold e-swap-3-dip must have exactly the fields",
        ),
        (
            lambda matrix: verdict_row(matrix, "e-swap-3-dip").update(pilot_data_visible="maybe"),
            "verdict threshold e-swap-3-dip is malformed",
        ),
        (
            lambda matrix: matrix["experiments"]["e-perf-10"]["capacity_envelope"].update(
                competitive_ratio_threshold=0.8
            ),
            "verdict threshold e-perf-10-competitive-ratio disagrees with the value the experiment declares",
        ),
    ],
)
def test_matrix_rejects_a_missing_malformed_or_drifted_threshold_table(mutate, message: str) -> None:
    matrix = json.loads(MATRIX.read_text())
    mutate(matrix)
    with tempfile.TemporaryDirectory() as tmp:
        path = Path(tmp) / "matrix.json"
        write_json(path, matrix)
        result = run_validator("matrix", str(path))
    assert result.returncode == 1
    assert message in result.stderr


@pytest.mark.parametrize(
    ("mutate", "message"),
    [
        (
            lambda matrix: matrix.pop("replication_concordance"),
            "replication_concordance must be an object",
        ),
        (
            lambda matrix: matrix["hosts"]["jetson"].update(role="canonical"),
            "replication_hosts must list every replication host",
        ),
    ],
)
def test_matrix_rejects_a_missing_or_drifted_replication_rule(mutate, message: str) -> None:
    matrix = json.loads(MATRIX.read_text())
    mutate(matrix)
    with tempfile.TemporaryDirectory() as tmp:
        path = Path(tmp) / "matrix.json"
        write_json(path, matrix)
        result = run_validator("matrix", str(path))
    assert result.returncode == 1
    assert message in result.stderr


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
        "isolated_cpus": "",
        "housekeeping_cpus": "0",
        "irq_default_cpus": "0",
        "throttled": "0x0",
        "broker_ready": True,
        "ekuiper_ready": True,
        "ekuiper_version": "2.1.5",
    }


def test_preflight_accepts_canonical_facts() -> None:
    with tempfile.TemporaryDirectory() as tmp:
        path = Path(tmp) / "facts.json"
        write_json(path, valid_facts())
        result = run_validator("preflight", str(path), "--require-ekuiper")
    assert result.returncode == 0, result.stderr
    assert "canonical preflight: PASS" in result.stdout


def test_preflight_requires_the_comparator_config_version(tmp_path: Path) -> None:
    comparator = tomllib.loads(
        (ROOT / "eval/configs/canonical/e-perf-1-ekuiper.toml").read_text()
    )["comparator"]
    path = tmp_path / "facts.json"
    write_json(path, {**valid_facts(), "ekuiper_version": comparator["version"]})

    result = run_validator("preflight", str(path), "--require-ekuiper")

    assert result.returncode == 0, result.stderr


def test_preflight_accepts_an_untagged_clean_source(tmp_path: Path) -> None:
    path = tmp_path / "facts.json"
    write_json(path, {**valid_facts(), "git_tags": []})

    result = run_validator("preflight", str(path), "--require-ekuiper")

    assert result.returncode == 0, result.stderr


def test_preflight_rejects_each_provenance_and_host_violation() -> None:
    invalid = {
        "dirty source": ("git_dirty", True),
        "host tag": ("host_tag", "shakedown-macos"),
        "CPU governor": ("cpu_governors", ["ondemand"]),
        "isolated CPUs": ("isolated_cpus", "1-3"),
        "housekeeping CPUs": ("housekeeping_cpus", "0-3"),
        "default IRQ CPUs": ("irq_default_cpus", "0-3"),
        "throttling": ("throttled", "0x50000"),
        "broker": ("broker_ready", False),
        "eKuiper": ("ekuiper_ready", False),
        "eKuiper version": ("ekuiper_version", "2.1.0"),
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
