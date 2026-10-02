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
    assert "schedule_records=2351" in result.stdout
    assert "measured_leaves=2121" in result.stdout


def test_final_campaign_policy_is_frozen_in_matrix() -> None:
    matrix = json.loads(MATRIX.read_text())
    campaign = matrix["final_campaign"]
    sweep = matrix["experiments"]["e-perf-10"]

    assert campaign["status"] == "frozen-before-execution"
    assert campaign["seed"] == 1729
    assert campaign["thesis_evidence"] is True
    assert campaign["expected_schedule_records"] == 2351
    assert campaign["expected_measured_leaves"] == 2121
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
    assert campaign["attempt_policy"] == {
        "infrastructure_retries": 1,
        "gate_experiments": ["e-val-1"],
    }
    assert campaign["mqtt_drain_grace_secs"] == 5
    assert len(campaign["wafer_config_catalog"]) == 55
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
    assert sweep["capacity_envelope"]["delivery_ceiling"].startswith("bracketed by tested rates")
    assert sweep["capacity_envelope"]["competitive_decision"] == (
        "PASS if WAFER lower bound / eKuiper upper bound >= threshold; "
        "FAIL if WAFER upper bound / eKuiper lower bound < threshold; otherwise CENSORED"
    )
    assert {"publisher-summary.json", "capacity-run.json", "subscriber-metadata.json"} <= set(
        sweep["required_outputs"]
    )

    assert matrix["experiments"]["e-perf-9"]["cache_scope"] == "linux-filesystem-page-cache"
    assert matrix["experiments"]["e-perf-5"]["incomplete_until"] == "matching x86 Linux batch"
    swap3 = matrix["experiments"]["e-swap-3"]
    assert swap3["conditions"] == [
        "wafer-hotswap",
        "wafer-restart",
        "ekuiper-restart",
        "ekuiper-make-before-break",
    ]
    assert set(swap3["required_outputs"]) == {
        "latency.hdr",
        "throughput.csv",
        "sequence.csv",
        "publisher-summary.json",
        "subscriber-metadata.json",
        "throughput-buckets.json",
        "throughput-buckets-10ms.json",
        "disruption-timeline.json",
        "disruption-analysis.json",
    }
    burst = matrix["experiments"]["e-swap-4"]
    assert "throughput-buckets-10ms.json" in burst["required_outputs"]
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
    assert {
        "burst-source-timing.json",
        "burst-source-summary.json",
        "swap-actual-t0.json",
    } <= set(burst["required_outputs"])
    assert burst["sink_tail_policy"] == {
        "alignment_clock": "unix-epoch-source-sink-alignment",
        "primary_start_secs": 0,
        "primary_end_secs": 120,
        "primary_bucket_count": 1200,
        "drain_start_secs": 120,
        "drain_end_secs": 130,
        "drain_bucket_count": 100,
        "bucket_width_ms": 100,
        "after_drain_events_allowed": 0,
        "source_completion_deadline_secs": 130,
        "require_full_sequence_reconciliation": True,
    }
    for experiment in ("e-swap-1", "e-swap-2", "e-swap-5", "e-swap-6"):
        definition = matrix["experiments"][experiment]
        assert definition["sample_unit"] == "run"
        assert definition["repetitions"] == 10
        assert definition["events_per_run"] == 50
        assert definition["independent_unit"] == "complete process run"
        assert "within run" in definition["nested_unit"]
    assert burst["independent_unit"] == "complete process run"
    assert burst["nested_unit"] == "one swap within run"
    assert all(
        definition["thesis_evidence"] is True
        for definition in matrix["experiments"].values()
    )


def test_independent_swap_and_rollback_candidate_contracts_are_frozen() -> None:
    experiments = json.loads(MATRIX.read_text())["enhanced_candidate"]["experiments"]
    swaps = experiments["e-swap-independent-sessions"]
    rollbacks = experiments["e-swap-rollback-sessions"]

    common = {
        "evidence_class": "candidate-supplementary",
        "thesis_evidence": False,
        "n30_admitted": False,
        "sample_unit": "independent host run",
        "repetitions": 5,
        "events_per_run": 50,
        "event_classes": ["first-use-aot", "cached"],
        "warmup_secs": 30,
        "rate_msg_s": 1_000,
        "payload_bytes": 128,
        "ordering": {"method": "seeded run order", "default_seed": 1729},
    }
    assert {key: swaps[key] for key in common} == common
    assert {key: rollbacks[key] for key in common} == common
    assert swaps["conditions"] == ["steady"]
    assert swaps["measurement_secs"] == 120
    assert swaps["no_pool_with"] == [
        "e-swap-1",
        "e-swap-2",
        "e-swap-6",
        "prior diagnostic rehearsals",
    ]
    assert rollbacks["conditions"] == ["process-trap-rollback"]
    assert rollbacks["measurement_secs"] == 300
    assert rollbacks["no_pool_with"] == ["e-swap-5", "prior diagnostic rehearsals"]
    assert "throughput-buckets-10ms.json" not in swaps["required_outputs"]
    assert "throughput-buckets-10ms.json" not in rollbacks["required_outputs"]
    assert "swap_timeline.json" not in rollbacks["required_outputs"]


def test_capacity_knee_candidate_contract_and_profile_are_frozen() -> None:
    candidate = json.loads(MATRIX.read_text())["enhanced_candidate"]["experiments"][
        "e-perf-capacity-knee"
    ]
    profile = tomllib.loads((ROOT / candidate["loadgen_profile"]).read_text())
    expected_grid = {
        "mqtt-loopback": [*range(4_000, 16_000, 1_000), 15_250, 15_500, 15_750, 16_000],
        "native": list(range(8_000, 16_000, 1_000)),
        "wafer": list(range(8_000, 16_000, 1_000)),
        "ekuiper": list(range(4_000, 9_000, 1_000)),
    }

    assert candidate["condition_grid_msg_s"] == expected_grid
    assert candidate["repetitions"] == 5
    assert candidate["warmup_secs"] == 30
    assert candidate["measurement_secs"] == 60
    assert candidate["thesis_evidence"] is False
    assert candidate["n30_admitted"] is False
    assert candidate["ordering"] == {
        "method": "seeded rate blocks with five-run balanced system order",
        "default_seed": 1729,
        "cooldown_secs": 60,
    }
    assert candidate["delivery_good"] == {
        "loss_aggregation": "sum(total_undelivered) / sum(intended)",
        "max_loss_percent": 1.0,
        "achieved_aggregation": "mean(run achieved_rate / intended_rate)",
        "min_achieved_ratio": 0.99,
        "duplicates_allowed": 0,
        "support_path_censoring": "mqtt-loopback",
    }
    assert profile["sweep"] == {
        "repetitions": 5,
        "ordering": "seeded rate blocks with five-run balanced system order",
        "cooldown_secs": 60,
        "loss_aggregation": "sum(total_undelivered) / sum(intended)",
        "max_loss_percent": 1.0,
        "achieved_aggregation": "mean(run achieved_rate / intended_rate)",
        "min_achieved_ratio": 0.99,
        "duplicates_allowed": 0,
        "support_path_censoring": "mqtt-loopback",
        "thesis_evidence": False,
        "n30_admitted": False,
    }
    assert profile["condition_grid_msg_s"] == expected_grid


def test_final_matrix_rejects_capacity_or_burst_drift() -> None:
    mutations = (
        ("e-perf-10 repetitions", lambda value: value["experiments"]["e-perf-10"].update(repetitions=29)),
        ("e-swap-4 repetitions", lambda value: value["experiments"]["e-swap-4"].update(repetitions=29)),
        ("e-perf-10 rate grid", lambda value: value["experiments"]["e-perf-10"].update(rate_points_msg_s=[1000, 4000])),
        ("e-swap-4 sample_unit", lambda value: value["experiments"]["e-swap-4"].update(sample_unit="")),
        (
            "e-swap-4 sink tail policy",
            lambda value: value["experiments"]["e-swap-4"]["sink_tail_policy"].update(
                drain_end_secs=131
            ),
        ),
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


@pytest.mark.parametrize(
    "policy",
    [
        {"infrastructure_retries": 2, "gate_experiments": ["e-val-1"]},
        {"infrastructure_retries": 1, "gate_experiments": []},
        None,
    ],
)
def test_final_matrix_rejects_attempt_policy_drift(policy: dict | None) -> None:
    matrix = json.loads(MATRIX.read_text())
    matrix["final_campaign"]["attempt_policy"] = policy
    with tempfile.TemporaryDirectory() as tmp:
        path = Path(tmp) / "matrix.json"
        write_json(path, matrix)
        result = run_validator("matrix", str(path))
    assert result.returncode == 1
    assert "attempt policy" in result.stderr


@pytest.mark.parametrize("grace", [0, 60, "5", None])
def test_final_matrix_rejects_mqtt_drain_grace_drift(grace: object) -> None:
    matrix = json.loads(MATRIX.read_text())
    matrix["final_campaign"]["mqtt_drain_grace_secs"] = grace
    with tempfile.TemporaryDirectory() as tmp:
        path = Path(tmp) / "matrix.json"
        write_json(path, matrix)
        result = run_validator("matrix", str(path))
    assert result.returncode == 1
    assert "MQTT drain grace" in result.stderr


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


def test_backpressure_freezes_policy_specific_internal_queue_contract() -> None:
    matrix = json.loads(MATRIX.read_text())
    experiment = matrix["experiments"]["e-backpressure"]
    expected_configs = {
        "slow": "eval/configs/e-backpressure/pipeline-saturated.toml",
        "drop": "eval/configs/e-backpressure/pipeline-drop.toml",
        "dead-letter": "eval/configs/e-backpressure/pipeline-dead-letter.toml",
    }

    assert experiment["conditions"] == ["slow", "drop", "dead-letter"]
    assert experiment["policy_configs"] == expected_configs
    assert experiment["measured_queue"] == "slow"
    assert experiment["queue_occupancy_threshold"] == 0.8
    assert experiment["queue_recovery_threshold"] == 0.1
    assert experiment["rss_limit_bytes"] == 268_435_456
    assert {"queue-depth.csv", "backpressure.json", "memory.csv", "sequence.csv"} <= set(
        experiment["required_outputs"]
    )

    for policy, path in expected_configs.items():
        config = tomllib.loads((ROOT / path).read_text())
        assert config["nodes"]["source"]["kind"] == "bench-source"
        assert config["nodes"]["source"]["rate"] == 1000.0
        assert config["nodes"]["slow"]["config"]["delay_ms"] == 5
        assert config["edges"][0]["capacity"] == 64
        assert config["edges"][0]["overflow"] == policy
        assert config["edges"][1]["overflow"] == "slow"
        assert config["dead_letter"]["kind"] == "file", "every run records dead-lettered messages"


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


def test_matrix_declares_every_verdict_threshold_with_its_origin() -> None:
    matrix = json.loads(MATRIX.read_text())
    rules = matrix["verdict_rules"]
    assert rules["one_sided_confidence"] == 0.95
    by_criterion = {row["criterion"]: row for row in rules["thresholds"]}
    assert by_criterion["e-perf-1-p95-ratio"]["value"] == 2.0
    assert by_criterion["e-perf-4-boundary-p50"]["role"] == "reference"
    assert by_criterion["e-perf-10-competitive-ratio"]["rule"] == "tested-rate-bracket"
    assert by_criterion["e-swap-4-p95-gap"]["value"] == 100_000_000
    assert all(
        row["value"] == 0 for row in rules["thresholds"] if row["rule"] == "exact-count"
    )
    assert all(row["origin"] and row["pilot_data_visible"] in (True, False, "unknown") for row in rules["thresholds"])


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


@pytest.mark.parametrize("offset", [None, -2, 0])
def test_final_matrix_rejects_a_moved_or_missing_placebo_offset(offset: int | None) -> None:
    matrix = json.loads(MATRIX.read_text())
    alignment = matrix["experiments"]["e-swap-3"]["event_alignment"]
    if offset is None:
        alignment.pop("placebo_offset_secs")
    else:
        alignment["placebo_offset_secs"] = offset
    with tempfile.TemporaryDirectory() as tmp:
        path = Path(tmp) / "matrix.json"
        write_json(path, matrix)
        result = run_validator("matrix", str(path))
    assert result.returncode == 1
    assert "e-swap-3 event alignment differs from the frozen estimator" in result.stderr


@pytest.mark.parametrize(
    ("mutate", "message"),
    [
        (lambda matrix: matrix.pop("verdict_rules"), "verdict_rules must be an object"),
        (
            lambda matrix: matrix["verdict_rules"].update(thresholds=[]),
            "verdict_rules thresholds must be a non-empty list",
        ),
        (
            lambda matrix: verdict_row(matrix, "e-swap-4-p95-gap").update(value=200_000_000),
            "verdict threshold e-swap-4-p95-gap differs from the frozen table",
        ),
        (
            lambda matrix: verdict_row(matrix, "e-perf-1-p95-ratio").update(direction="<"),
            "verdict threshold e-perf-1-p95-ratio differs from the frozen table",
        ),
        (
            lambda matrix: verdict_row(matrix, "e-iso-containment").update(rule="one-sided-bound"),
            "verdict threshold e-iso-containment differs from the frozen table",
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
            lambda matrix: matrix["verdict_rules"]["thresholds"].remove(
                verdict_row(matrix, "e-iso-7-throughput-drop")
            ),
            "verdict threshold e-iso-7-throughput-drop is missing",
        ),
        (
            lambda matrix: matrix["verdict_rules"].update(one_sided_confidence=0.9),
            "one_sided_confidence must be 0.95",
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


def test_density_requires_the_measured_container_floor() -> None:
    matrix = json.loads(MATRIX.read_text())
    matrix["experiments"]["e-density-1"]["required_outputs"] = ["binary-sizes.csv"]
    with tempfile.TemporaryDirectory() as tmp:
        path = Path(tmp) / "matrix.json"
        write_json(path, matrix)
        result = run_validator("matrix", str(path))
    assert result.returncode == 1
    assert "e-density-1 required outputs differ" in result.stderr


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
