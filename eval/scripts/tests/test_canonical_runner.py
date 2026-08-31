#!/usr/bin/env python3

import json
import sys
import tempfile
import tomllib
from pathlib import Path

import pytest

ROOT = Path(__file__).resolve().parents[3]
sys.path.insert(0, str(ROOT / "eval/scripts/lib"))

from canonical_runner import (  # noqa: E402
    build_schedule,
    derive_containment,
    evaluate_validation_gate,
    postprocess_run,
    select_attempt,
    summarize_recovery,
    write_progress,
)

T2_EXPERIMENTS = {
    "e-val-1",
    "e-perf-3",
    "e-perf-4",
    "e-perf-6",
    "e-perf-8",
    "e-perf-7",
    "e-perf-9",
    "e-backpressure",
}
T3_EXPERIMENTS = {"e-perf-1", "e-perf-2", "e-perf-5", "e-swap-3"}
T4_EXPERIMENTS = {
    *(f"e-iso-{index}" for index in range(1, 9)),
    *(f"e-swap-{index}" for index in range(1, 7)),
}


def test_schedule_covers_performance_matrix() -> None:
    schedule = build_schedule(T2_EXPERIMENTS, seed=1729)
    by_experiment: dict[str, list] = {}
    for item in schedule:
        by_experiment.setdefault(item.experiment, []).append(item)

    assert set(by_experiment) == T2_EXPERIMENTS
    assert schedule[0].experiment == "e-val-1"
    assert len(by_experiment["e-val-1"]) == 30
    assert len(by_experiment["e-perf-3"]) == 4 * 30
    assert len(by_experiment["e-perf-4"]) == 4 * 30
    assert len(by_experiment["e-perf-6"]) == 4 * 30
    assert len(by_experiment["e-perf-8"]) == 0 or all(
        item.shared_from == "e-perf-6" for item in by_experiment["e-perf-8"]
    )
    assert len(by_experiment["e-perf-7"]) == 4 * 30
    assert len(by_experiment["e-perf-9"]) == 6 * 30
    assert len(by_experiment["e-backpressure"]) == 30
    assert all(item.warmup_secs == 30 for item in by_experiment["e-perf-4"])
    assert all(item.runtime_cpus == "1-3" for item in schedule)
    assert all(item.support_cpus == "0" for item in schedule)
    assert len({item.result_key for item in schedule}) == len(schedule)


def test_comparator_schedule_is_native_and_cross_arch_is_explicitly_partial() -> None:
    schedule = build_schedule(T3_EXPERIMENTS, seed=1729)
    by_experiment: dict[str, list] = {}
    for item in schedule:
        by_experiment.setdefault(item.experiment, []).append(item)

    assert len(by_experiment["e-perf-1"]) == 3 * 30
    assert {item.system for item in by_experiment["e-perf-1"]} == {
        "wafer",
        "native",
        "ekuiper",
    }
    assert len(by_experiment["e-perf-2"]) == 3 * 30
    assert all(item.shared_from == "e-perf-1" for item in by_experiment["e-perf-2"])
    assert len(by_experiment["e-perf-5"]) == 2 * 30
    assert {item.system for item in by_experiment["e-perf-5"]} == {"wafer", "native"}
    assert len(by_experiment["e-swap-3"]) == 3 * 30
    assert all("docker" not in item.config for item in schedule)
    assert "docker" not in (ROOT / "eval/scripts/lib/canonical_runner.py").read_text().lower()


def test_isolation_and_swap_schedule_preserves_experiment_semantics() -> None:
    schedule = build_schedule(T4_EXPERIMENTS, seed=1729)
    by_experiment: dict[str, list] = {}
    for item in schedule:
        by_experiment.setdefault(item.experiment, []).append(item)

    assert set(by_experiment) == T4_EXPERIMENTS
    for index in range(1, 9):
        assert len(by_experiment[f"e-iso-{index}"]) >= 30
    assert all(item.warmup_secs == 0 for index in range(1, 7) for item in by_experiment[f"e-iso-{index}"])
    assert len(by_experiment["e-swap-1"]) == 1
    assert len(by_experiment["e-swap-2"]) == 1
    assert len(by_experiment["e-swap-4"]) == 1
    assert len(by_experiment["e-swap-5"]) == 1
    assert len(by_experiment["e-swap-6"]) == 1
    assert by_experiment["e-swap-2"][0].shared_from == "e-swap-1"
    assert by_experiment["e-swap-6"][0].shared_from == "e-swap-1"
    assert all("shakedown-macos" not in item.result_key for item in schedule)

    matrix = json.loads((ROOT / "eval/canonical-matrix.json").read_text())["experiments"]
    for index in range(1, 9):
        assert "per_node_metrics.csv" in matrix[f"e-iso-{index}"]["required_outputs"]
    assert {"recovery.csv", "recovery.json"} <= set(matrix["e-iso-8"]["required_outputs"])
    for index in range(1, 7):
        assert "swap_timeline.json" in matrix[f"e-swap-{index}"]["required_outputs"]
    assert "rollback.json" in matrix["e-swap-5"]["required_outputs"]


def test_isolation_derivations_use_raw_runtime_metrics() -> None:
    with tempfile.TemporaryDirectory() as tmp:
        root = Path(tmp)
        (root / "per_node_metrics.csv").write_text(
            "node_id,messages_in,messages_out,traps_total,error_state_seconds,recovery_count\n"
            "source,12000,12000,0,0,0\n"
            "attack,12000,0,12000,1.2,12000\n"
            "sink,0,0,0,0,0\n"
        )
        (root / "stdout.log").write_text("expected guest traps only\n")
        containment = derive_containment(root)
        assert containment["contained"] is True
        assert containment["traps_total"] == 12_000
        assert containment["runtime_panic"] is False

        (root / "recovery.csv").write_text(
            "node_id,sample_index,duration_ns\n"
            "attack,0,100\nattack,1,200\nattack,2,1000\n"
        )
        recovery = summarize_recovery(root / "recovery.csv")
        assert recovery["sample_count"] == 3
        assert recovery["p50_ns"] == 200
        assert recovery["p99_ns"] == 1000


def test_containment_derivation_rejects_placeholder_metrics() -> None:
    with tempfile.TemporaryDirectory() as tmp:
        root = Path(tmp)
        (root / "per_node_metrics.csv").write_text(
            "node_id,messages_in,messages_out,traps_total,error_state_seconds,recovery_count\n"
            "# runtime did not emit final metrics\n"
        )
        (root / "stdout.log").write_text("runtime required SIGKILL\n")
        with pytest.raises(ValueError, match="no runtime metric rows"):
            derive_containment(root)


def test_canonical_configs_match_frozen_windows() -> None:
    config_paths = {
        item.config
        for item in build_schedule(T2_EXPERIMENTS, seed=1729)
        if item.loadgen_profile is None and item.shared_from is None
    }
    for relative in config_paths:
        assert (ROOT / relative).is_file(), relative

    validation = tomllib.loads(
        (ROOT / "eval/configs/canonical/e-val-1.toml").read_text()
    )
    assert validation["nodes"]["source"]["total_messages"] == 900
    assert validation["nodes"]["source"]["warmup_messages"] == 300
    assert validation["nodes"]["sink"]["warmup_secs"] == 30

    for path in sorted((ROOT / "eval/configs/canonical").glob("e-perf-4-*.toml")):
        config = tomllib.loads(path.read_text())
        assert config["nodes"]["source"]["total_messages"] == 90_000
        assert config["nodes"]["source"]["warmup_messages"] == 30_000
        assert config["nodes"]["sink"]["warmup_secs"] == 30

    for relative in sorted(
        path
        for path in config_paths
        if "pipeline-depth-" in path or "pipeline-c-" in path
    ):
        config = tomllib.loads((ROOT / relative).read_text())
        source = config["nodes"]["source"]
        sink = config["nodes"]["sink"]
        assert source["total_messages"] == 90_000, relative
        assert source["warmup_messages"] == 30_000, relative
        assert sink["warmup_secs"] == 30, relative

    native = tomllib.loads((ROOT / "eval/configs/pipeline-d-native.toml").read_text())
    assert native["nodes"]["source"]["total_messages"] == 90_000
    assert native["nodes"]["source"]["warmup_messages"] == 30_000

    burst = tomllib.loads(
        (ROOT / "eval/loadgen/canonical-burst.toml").read_text()
    )["loadgen"]
    assert burst["duration_secs"] == 300
    assert burst["warmup_secs"] == 30


def test_infinite_loop_experiment_enables_epoch_interruption() -> None:
    config = tomllib.loads(
        (ROOT / "eval/configs/e-iso-4/pipeline.toml").read_text()
    )
    assert config["engine"]["epoch_deadline"] > 0


def test_eperf9_plugin_configs_supply_required_guest_configuration() -> None:
    paths = [
        ROOT / f"eval/configs/e-perf-9/pipeline-tier-{tier}.toml"
        for tier in ("small", "medium", "large")
    ]
    configs = [tomllib.loads(path.read_text()) for path in paths]
    assert all(config["nodes"]["source"]["total_messages"] == 1 for config in configs)

    medium = configs[1]["nodes"]["t1"]["config"]
    assert medium["fields"]

    large = configs[2]["nodes"]["t1"]["config"]
    assert large["sample_rate"] > 0
    assert large["fft_size"] > 0
    assert large["bands"]


def test_external_subscriber_percentiles_do_not_parse_binary_hdr() -> None:
    item = next(
        item
        for item in build_schedule({"e-perf-1"}, seed=1729)
        if item.condition == "wafer" and item.run_index == 1
    )
    with tempfile.TemporaryDirectory() as tmp:
        output = Path(tmp)
        (output / "metadata.json").write_text(
            json.dumps({"loadgen": {}, "system": "wafer"})
        )
        (output / "latency.hdr").write_bytes(b"\x1c\x84\x93\x03binary")
        (output / "subscriber-metadata.json").write_text(
            json.dumps(
                {
                    "started_at_ns": 1_000_000_000,
                    "ended_at_ns": 61_000_000_000,
                    "total_recorded": 60_000,
                    "latency_p50_ns": 1_000,
                    "latency_p95_ns": 2_000,
                    "latency_p99_ns": 3_000,
                    "latency_p999_ns": 4_000,
                }
            )
        )
        postprocess_run(ROOT, item, output)
        percentiles = json.loads((output / "percentiles.json").read_text())
    assert percentiles["p99_ns"] == 3_000
    assert percentiles["total_count"] == 60_000


def test_progress_log_records_counts_and_temperature_field() -> None:
    with tempfile.TemporaryDirectory() as tmp:
        ledger = Path(tmp)
        write_progress(
            ledger,
            "item-finished",
            completed=3,
            total=10,
            item="e-perf-1/wafer/run-01",
            failures=1,
        )
        entry = json.loads((ledger / "progress.jsonl").read_text())
    assert entry["event"] == "item-finished"
    assert entry["completed"] == 3
    assert entry["total"] == 10
    assert entry["item"] == "e-perf-1/wafer/run-01"
    assert entry["failures"] == 1
    assert "temperature_c" in entry


def test_resume_skips_passed_attempt_and_preserves_failed_attempt() -> None:
    with tempfile.TemporaryDirectory() as tmp:
        condition = Path(tmp)
        passed = condition / "run-01-attempt-01"
        passed.mkdir()
        (passed / "canonical-status.json").write_text(
            json.dumps({"status": "passed"})
        )
        selection = select_attempt(condition, 1)
        assert selection.skip is True
        assert selection.path == passed

        failed = condition / "run-02-attempt-01"
        failed.mkdir()
        (failed / "canonical-status.json").write_text(
            json.dumps({"status": "failed"})
        )
        selection = select_attempt(condition, 2)
        assert selection.skip is False
        assert selection.path.name == "run-02-attempt-02"
        assert failed.exists()


def test_validation_gate_rejects_one_bad_repetition() -> None:
    with tempfile.TemporaryDirectory() as tmp:
        root = Path(tmp)
        for index in range(1, 31):
            run = root / f"run-{index:02d}-attempt-01"
            run.mkdir()
            p99_ns = 51_000_000 if index != 17 else 60_000_000
            (run / "percentiles.json").write_text(
                json.dumps({"total_count": 600, "p99_ns": p99_ns})
            )
            (run / "canonical-status.json").write_text(
                json.dumps({"status": "passed"})
            )
        result = evaluate_validation_gate(root, expected_runs=30)
    assert result.passed is False
    assert result.failed_runs == [17]


def test_validation_gate_accepts_all_repetitions() -> None:
    with tempfile.TemporaryDirectory() as tmp:
        root = Path(tmp)
        for index in range(1, 31):
            run = root / f"run-{index:02d}-attempt-01"
            run.mkdir()
            (run / "percentiles.json").write_text(
                json.dumps({"total_count": 600, "p99_ns": 51_000_000})
            )
            (run / "canonical-status.json").write_text(
                json.dumps({"status": "passed"})
            )
        result = evaluate_validation_gate(root, expected_runs=30)
    assert result.passed is True
    assert result.failed_runs == []


if __name__ == "__main__":
    test_schedule_covers_performance_matrix()
    test_comparator_schedule_is_native_and_cross_arch_is_explicitly_partial()
    test_isolation_and_swap_schedule_preserves_experiment_semantics()
    test_isolation_derivations_use_raw_runtime_metrics()
    test_canonical_configs_match_frozen_windows()
    test_external_subscriber_percentiles_do_not_parse_binary_hdr()
    test_progress_log_records_counts_and_temperature_field()
    test_resume_skips_passed_attempt_and_preserves_failed_attempt()
    test_validation_gate_rejects_one_bad_repetition()
    test_validation_gate_accepts_all_repetitions()
    print("canonical runner tests: PASS")
