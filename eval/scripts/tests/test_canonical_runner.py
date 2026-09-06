#!/usr/bin/env python3

import hashlib
import json
import subprocess
import sys
import tempfile
import tomllib
from pathlib import Path

import pytest

ROOT = Path(__file__).resolve().parents[3]
sys.path.insert(0, str(ROOT / "eval/scripts/lib"))

import canonical_runner as runner  # noqa: E402
from canonical_runner import (  # noqa: E402
    analyze_backpressure,
    analyze_capacity_scout_summary,
    analyze_swap3_disruption,
    analyze_rate_sweep_traces,
    estimate_capacity_envelope,
    build_capacity_scout_invocation,
    build_capacity_scout_rate_block,
    build_swap3_invocation,
    build_swap4_timeline,
    capacity_scout_failed_attempt_stop_reason,
    capacity_scout_next_rate,
    capacity_scout_safety_action,
    capacity_scout_source_state,
    persist_capacity_scout_decision,
    replay_capacity_scout_decisions,
    build_focused_schedule,
    build_schedule,
    classify_capacity_scout_probe,
    classify_sustainable_throughput,
    compare_branch_a,
    compare_branch_conditions,
    copy_shared_result,
    derive_branch_isolation,
    derive_hotswap_evidence,
    derive_containment,
    evaluate_validation_gate,
    load_capacity_scout_replay,
    hot_swap_offsets,
    loadgen_command,
    postprocess_run,
    CAPACITY_SCOUT_SYSTEMS,
    ProcessResourceSampler,
    RunItem,
    select_attempt,
    summarize_branch_isolation,
    summarize_process_resources,
    summarize_swap4_runs,
    summarize_rate_sweep,
    summarize_recovery,
    validate_backpressure_result,
    validate_capacity_scout_decision_replay,
    validate_capacity_run_result,
    validate_capacity_scout_result,
    verify_capacity_result_files,
    validate_ekuiper_process_snapshot,
    verify_capacity_scout_result_files,
    write_capacity_scout_progress,
    write_capacity_result,
    write_capacity_scout_result,
    validate_focused_freeze,
    validate_rate_sweep_result,
    validate_startup_artifact,
    validate_swap3_artifacts,
    validate_swap4_artifacts,
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


def test_validation_dry_run_uses_a_dedicated_result_directory() -> None:
    completed = subprocess.run(
        [str(ROOT / "eval/scripts/run-rpi5-validation.sh"), "--dry-run"],
        cwd=ROOT,
        capture_output=True,
        text=True,
        check=False,
    )
    assert completed.returncode == 0, completed.stdout + completed.stderr
    assert "--output-dir" in completed.stdout
    assert "eval/results/e-val-1/rpi5-validation-" in completed.stdout


def test_density_dispatches_static_collector_before_generic_config(
    monkeypatch: pytest.MonkeyPatch, tmp_path: Path
) -> None:
    item = RunItem(
        experiment="e-density-1",
        condition="release-components",
        run_index=1,
        config="",
        warmup_secs=0,
        measurement_secs=0,
        system="static",
    )
    observed = []
    monkeypatch.setattr(
        runner,
        "run_density_item",
        lambda root, candidate, selection: observed.append((root, candidate, selection)) or True,
        raising=False,
    )
    monkeypatch.setattr(runner, "set_ekuiper_active", lambda root, active: None)

    assert runner.run_item(tmp_path, "test", item)
    assert len(observed) == 1


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
    matrix = json.loads((ROOT / "eval/canonical-matrix.json").read_text())
    assert set(matrix["experiments"]["e-perf-9"]["required_outputs"]) == {
        "startup-preparation.json",
        "startup.json",
    }
    assert len(by_experiment["e-backpressure"]) == 30
    assert all(item.warmup_secs == 30 for item in by_experiment["e-perf-4"])
    assert all(item.runtime_cpus == "1-3" for item in schedule)
    assert all(item.support_cpus == "0" for item in schedule)
    assert len({item.result_key for item in schedule}) == len(schedule)
    catalog = {
        (entry["experiment"], entry["condition"]): entry["config"]
        for entry in matrix["final_campaign"]["wafer_config_catalog"]
    }
    assert all(
        item.config == catalog[(item.experiment, item.condition)]
        for item in schedule
        if item.system == "wafer" and item.config
    )


def test_final_wafer_catalog_exactly_matches_runner_schedule() -> None:
    matrix = json.loads((ROOT / "eval/canonical-matrix.json").read_text())
    schedule = build_schedule(set(matrix["experiments"]), seed=matrix["final_campaign"]["seed"])
    expected = {
        (item.experiment, item.condition, item.config)
        for item in schedule
        if item.system == "wafer" and item.config
    }
    catalog = {
        (entry["experiment"], entry["condition"], entry["config"])
        for entry in matrix["final_campaign"]["wafer_config_catalog"]
    }
    assert catalog == expected


def test_focused_schedule_matches_frozen_condition_runs() -> None:
    matrix = json.loads((ROOT / "eval/canonical-matrix.json").read_text())
    selected = matrix["focused_pilot"]["experiments"]
    expected = {
        f"{experiment}/{condition}/run-{run_index:02d}"
        for experiment, definition in selected.items()
        for condition, run_indices in definition["condition_runs"].items()
        for run_index in run_indices
    }

    schedule = build_focused_schedule(seed=matrix["focused_pilot"]["seed"])

    assert {item.result_key for item in schedule} == expected
    assert len(schedule) == len(expected) == 137
    assert [item.run_index for item in schedule if item.experiment == "e-swap-3"] == [1, 2, 3]
    assert {item.condition for item in schedule if item.experiment == "e-swap-3"} == {
        "wafer-hotswap"
    }
    assert len([item for item in schedule if item.experiment == "e-swap-5"]) == 1
    assert all(item.system != "ekuiper" or item.experiment == "e-perf-10" for item in schedule)


def test_focused_freeze_matches_canonical_matrix_and_schedule() -> None:
    receipt = validate_focused_freeze(ROOT, ROOT / "eval/canonical-matrix.json")
    schedule_path = ROOT / "eval/focused-pilot-schedule.json"
    schedule_bytes = schedule_path.read_bytes()
    schedule = json.loads(schedule_bytes)
    expected = [item.__dict__ for item in build_focused_schedule(seed=1729)]

    assert schedule == expected
    assert receipt["selected_leaf_count"] == len(schedule) == 137
    assert receipt["schedule_sha256"] == hashlib.sha256(schedule_bytes).hexdigest()
    assert receipt["thesis_evidence"] is False


def test_focused_freeze_rejects_matrix_drift(tmp_path: Path) -> None:
    matrix = tmp_path / "eval/canonical-matrix.json"
    matrix.parent.mkdir(parents=True)
    matrix.write_text('{"focused_pilot": {}}\n')
    receipt = tmp_path / "eval/focused-pilot-freeze.json"
    receipt.write_text(json.dumps({
        "status": "frozen-before-execution",
        "thesis_evidence": False,
        "canonical_matrix_sha256": "0" * 64,
    }))

    with pytest.raises(ValueError, match="matrix changed after freeze"):
        validate_focused_freeze(tmp_path, matrix)


def test_hotswap_evidence_keeps_internal_and_sink_timings_distinct() -> None:
    requests = [
        {
            "event_index": 0,
            "request_duration_ns": 120_000_000,
            "request_duration_clock": "monotonic",
            "http_status": 200,
            "body": {
                "timeline": {
                    "compile_ns": 100_000_000,
                    "instantiate_ns": 10_000_000,
                    "signal_ns": 1_000,
                    "ack_ns": 2_000_000,
                    "convergence_ns": 3_000_000,
                }
            },
        },
        {
            "event_index": 1,
            "request_duration_ns": 2_000_000,
            "request_duration_clock": "monotonic",
            "http_status": 200,
            "body": {
                "timeline": {
                    "compile_ns": 100_000,
                    "instantiate_ns": 200_000,
                    "signal_ns": 1_000,
                    "ack_ns": 300_000,
                    "convergence_ns": 400_000,
                }
            },
        },
    ]
    sink = {"transitions": [{"pause_ns": 0}, {"pause_ns": 1_000_000}]}

    evidence = derive_hotswap_evidence(
        requests,
        sink,
        experiment="e-swap-4",
        condition="burst-2x",
        source_leaf="eval/results/e-swap-4/batch/burst-2x/run-01-attempt-01",
    )

    assert evidence["events"][0]["http_total_ns"] == 120_000_000
    assert evidence["events"][0]["sink_observed_output_gap_ns"] == 0
    assert evidence["events"][1]["compile_ns"] == 100_000
    assert evidence["events"][1]["sink_observed_output_gap_ns"] == 1_000_000
    assert "does not imply" in evidence["interpretation"]
    assert evidence["measurement_source_leaf"].startswith("eval/results/e-swap-4/")


def test_hotswap_evidence_rejects_unit_name_conflation() -> None:
    request = {
        "event_index": 0,
        "request_duration_ns": 2_000_000,
        "http_status": 200,
        "body": {
            "timeline": {
                "compile_ms": 1.0,
                "instantiate_ns": 1,
                "signal_ns": 1,
                "ack_ns": 1,
                "convergence_ns": 1,
            }
        },
    }
    with pytest.raises(ValueError, match="compile_ns"):
        derive_hotswap_evidence(
            [request],
            {"transitions": [{"pause_ns": 1}]},
            experiment="e-swap-1",
            condition="steady",
            source_leaf="source",
        )


def test_shared_hotswap_result_preserves_single_source_leaf(tmp_path: Path) -> None:
    source = tmp_path / "eval/results/e-swap-1/rpi5-batch/steady/run-01-attempt-01"
    source.mkdir(parents=True)
    (source / "canonical-status.json").write_text('{"status":"passed"}')
    (source / "metadata.json").write_text(
        json.dumps({"experiment": "e-swap-1", "measurement_source_leaf": str(source.relative_to(tmp_path))})
    )
    (source / "hotswap-analysis.json").write_text(
        json.dumps({"experiment": "e-swap-1", "measurement_source_leaf": str(source.relative_to(tmp_path)), "events": [{"event_index": 0}]})
    )
    item = RunItem(
        experiment="e-swap-2",
        condition="steady",
        run_index=1,
        config="unused.toml",
        warmup_secs=30,
        measurement_secs=120,
        shared_from="e-swap-1",
    )

    target = copy_shared_result(tmp_path, "batch", item)

    metadata = json.loads((target / "metadata.json").read_text())
    analysis = json.loads((target / "hotswap-analysis.json").read_text())
    expected_source = str(source.relative_to(tmp_path))
    assert metadata["shared_from"] == expected_source
    assert metadata["measurement_source_leaf"] == expected_source
    assert metadata["shared_measurement"] is True
    assert analysis["shared_from"] == expected_source
    assert analysis["measurement_source_leaf"] == expected_source
    assert analysis["experiment"] == "e-swap-2"


def swap3_fixture(rates: list[float] | None = None, *, received: int = 120_000) -> tuple[dict, dict, dict, dict]:
    rates = rates or [1_000.0] * 200
    buckets = []
    for index, rate in enumerate(rates):
        unique = int(rate / 10)
        start = -10_000_000_000 + index * 100_000_000
        buckets.append({
            "start_offset_ns": start,
            "end_offset_ns": start + 100_000_000,
            "received_unique": unique,
            "received_events": unique,
            "duplicates": 0,
            "rate_msg_s": rate,
        })
    throughput = {
        "schema_version": 1,
        "clock": "unix-epoch",
        "clock_purpose": "cross-process-alignment",
        "measurement_start_timestamp_ns": 1_000_000_000_000,
        "scheduled_event_timestamp_ns": 1_060_000_000_000,
        "scheduled_event_offset_ns": 60_000_000_000,
        "event_timestamp_ns": 1_060_005_000_000,
        "event_offset_from_measurement_start_ns": 60_005_000_000,
        "alignment_error_ns": 5_000_000,
        "alignment_tolerance_ns": 10_000_000,
        "bucket_width_ns": 100_000_000,
        "coverage_start_offset_ns": -10_000_000_000,
        "coverage_end_offset_ns": 10_000_000_000,
        "received_unique": sum(row["received_unique"] for row in buckets),
        "received_events": sum(row["received_events"] for row in buckets),
        "duplicates": 0,
        "buckets": buckets,
    }
    timeline = {
        "schema_version": 1,
        "strategy": "wafer-hotswap",
        "timestamp_clock": "unix-epoch",
        "timestamp_clock_purpose": "cross-process-alignment",
        "scheduling_clock": "monotonic",
        "duration_clock": "monotonic",
        "measurement_start_timestamp_ns": 1_000_000_000_000,
        "scheduled_event_timestamp_ns": 1_060_000_000_000,
        "scheduled_event_offset_ns": 60_000_000_000,
        "event_timestamp_ns": 1_060_005_000_000,
        "event_offset_from_measurement_start_ns": 60_005_000_000,
        "alignment_error_ns": 5_000_000,
        "alignment_tolerance_ns": 10_000_000,
        "action_start_timestamp_ns": 1_060_005_000_000,
        "action_end_timestamp_ns": 1_060_205_000_000,
        "action_end_offset_ns": 200_000_000,
        "action_start_monotonic_ns": 5_000_000_000,
        "action_end_monotonic_ns": 5_199_000_000,
        "action_duration_ns": 199_000_000,
    }
    publisher = {"intended": 120_000, "rejected": 0, "enqueued": 120_000}
    subscriber = {
        "total_recorded": received,
        "latency_p50_ns": 100,
        "latency_p95_ns": 200,
        "latency_p99_ns": 300,
        "sequence": {"total_received": received, "total_duplicates": 0},
    }
    return throughput, timeline, publisher, subscriber


def test_swap3_analysis_covers_no_partial_full_and_delayed_recovery() -> None:
    throughput, timeline, publisher, subscriber = swap3_fixture()
    validate_swap3_artifacts(throughput, timeline, publisher, subscriber)
    uninterrupted = analyze_swap3_disruption(throughput, timeline, publisher, subscriber)
    assert uninterrupted["dip_percent"] == 0
    assert uninterrupted["interruption_ns"] == 0
    assert uninterrupted["recovery_ns"] == 0

    partial_rates = [1_000.0] * 200
    partial_rates[98:103] = [500.0] * 5
    partial = analyze_swap3_disruption(*swap3_fixture(partial_rates))
    assert partial["dip_percent"] == 50
    assert partial["interruption_ns"] == 500_000_000
    assert partial["recovery_ns"] == 100_000_000

    full_rates = [1_000.0] * 200
    full_rates[80:130] = [0.0] * 50
    full = analyze_swap3_disruption(*swap3_fixture(full_rates))
    assert full["dip_percent"] == 100
    assert full["interruption_ns"] == 5_000_000_000

    pre_event_rates = [1_000.0] * 200
    pre_event_rates[95:100] = [0.0] * 5
    pre_event = analyze_swap3_disruption(*swap3_fixture(pre_event_rates))
    assert pre_event["interruption_ns"] == 0

    delayed_rates = [1_000.0] * 200
    delayed_rates[100:130] = [0.0] * 30
    delayed = analyze_swap3_disruption(*swap3_fixture(delayed_rates))
    assert delayed["recovery_ns"] == 2_800_000_000
    assert delayed["recovery_right_censored"] is False

    delayed_rates[100:] = [0.0] * 100
    censored = analyze_swap3_disruption(*swap3_fixture(delayed_rates))
    assert censored["recovery_ns"] == 9_800_000_000
    assert censored["recovery_right_censored"] is True


def test_swap3_analysis_reports_loss_and_validator_rejects_drift() -> None:
    throughput, timeline, publisher, subscriber = swap3_fixture(received=119_990)
    analysis = analyze_swap3_disruption(throughput, timeline, publisher, subscriber)
    assert analysis["loss"] == 10
    assert analysis["duplicates"] == 0
    assert analysis["latency_ns"] == {"p50": 100, "p95": 200, "p99": 300}

    invalid = json.loads(json.dumps(throughput))
    invalid["clock"] = "monotonic"
    with pytest.raises(ValueError, match="alignment clock"):
        validate_swap3_artifacts(invalid, timeline, publisher, subscriber)
    invalid = json.loads(json.dumps(throughput))
    invalid["buckets"][1]["start_offset_ns"] += 1
    with pytest.raises(ValueError, match="contiguous"):
        validate_swap3_artifacts(invalid, timeline, publisher, subscriber)
    invalid_timeline = json.loads(json.dumps(timeline))
    invalid_timeline["event_timestamp_ns"] += 6_000_000
    invalid_timeline["action_start_timestamp_ns"] += 6_000_000
    invalid_timeline["event_offset_from_measurement_start_ns"] += 6_000_000
    invalid_timeline["alignment_error_ns"] += 6_000_000
    with pytest.raises(ValueError, match="event clocks"):
        validate_swap3_artifacts(throughput, invalid_timeline, publisher, subscriber)
    invalid_publisher = {**publisher, "enqueued": 119_999}
    with pytest.raises(ValueError, match="totals do not reconcile"):
        validate_swap3_artifacts(throughput, timeline, invalid_publisher, subscriber)


def test_swap3_strategies_share_boundary_and_commands_except_strategy() -> None:
    items = [
        next(
            item for item in build_schedule({"e-swap-3"}, seed=1729)
            if item.condition == strategy and item.run_index == 1
        )
        for strategy in ("wafer-hotswap", "wafer-restart", "ekuiper-restart")
    ]
    invocations = [build_swap3_invocation(ROOT, item, Path("/tmp/swap3")) for item in items]
    assert all(invocation["controlled_factors"] == invocations[0]["controlled_factors"] for invocation in invocations)
    assert {invocation["strategy"] for invocation in invocations} == {
        "wafer-hotswap", "wafer-restart", "ekuiper-restart"
    }
    assert all("--timing-receipt" in invocation["publisher_command"] for invocation in invocations)
    assert all("--publisher-timing-receipt" in invocation["subscriber_command"] for invocation in invocations)
    assert all("--action-timing-receipt" in invocation["subscriber_command"] for invocation in invocations)
    assert all(invocation["controlled_factors"]["event_offset_ns"] == 60_000_000_000 for invocation in invocations)


def swap4_fixture() -> tuple[dict, dict, list[dict], dict, dict, dict]:
    measurement_start = 1_000_000_000_000
    timing = {
        "measurement_start_ns": measurement_start,
        "burst_start_ns": measurement_start + 55_000_000_000,
        "scheduled_swap_ns": measurement_start + 60_000_000_000,
        "burst_end_ns": measurement_start + 65_000_000_000,
        "scheduled_measurement_end_ns": measurement_start + 120_000_000_000,
    }
    source = {
        "measurement_start_ns": measurement_start,
        "measurement_end_ns": measurement_start + 120_001_000_000,
        "source_completion_offset_ns": 120_001_000_000,
        "rates_msg_s": [1_000.0, 2_000.0, 1_000.0],
        "phase_offsets_ns": [0, 55_000_000_000, 65_000_000_000, 120_000_000_000],
        "warmup_messages": 30_000,
        "intended_phase_messages": [55_000, 20_000, 55_000],
        "emitted_phase_messages": [55_000, 20_000, 55_000],
        "intended_measurement_messages": 130_000,
        "emitted_measurement_messages": 130_000,
        "total_emitted_messages": 160_000,
    }
    request = [{
        "event_index": 0,
        "request_started_ns": measurement_start + 60_005_000_000,
        "request_finished_ns": measurement_start + 60_010_000_000,
        "request_timestamp_clock": "unix-epoch",
        "request_duration_ns": 5_000_000,
        "request_duration_clock": "monotonic",
        "http_status": 200,
        "body": {"timeline": {field: 1 for field in ("compile_ns", "instantiate_ns", "signal_ns", "ack_ns", "convergence_ns")}},
    }]
    sink = {"transitions": [{"from": "v1", "to": "v2", "pause_ns": 80_000_000}]}
    buckets = []
    for index in range(1_200):
        count = 200 if 550 <= index < 650 else 100
        buckets.append({
            "start_offset_ns": index * 100_000_000,
            "end_offset_ns": (index + 1) * 100_000_000,
            "received_unique": count,
            "received_events": count,
            "duplicates": 0,
            "rate_msg_s": count * 10,
        })
    buckets[-1]["received_unique"] -= 1
    buckets[-1]["received_events"] -= 1
    buckets[-1]["rate_msg_s"] -= 10
    drain_buckets = [
        {
            "start_offset_ns": 120_000_000_000 + index * 100_000_000,
            "end_offset_ns": 120_000_000_000 + (index + 1) * 100_000_000,
            "received_unique": 1 if index == 0 else 0,
            "received_events": 1 if index == 0 else 0,
            "duplicates": 0,
            "rate_msg_s": 10 if index == 0 else 0,
        }
        for index in range(100)
    ]
    throughput = {
        "schema_version": 1,
        "clock": "unix-epoch-source-sink-alignment",
        "source_measurement_start_unix_ns": measurement_start,
        "origin_mismatch_events": 0,
        "missing_origin_events": 0,
        "bucket_width_ns": 100_000_000,
        "coverage_start_offset_ns": 0,
        "coverage_end_offset_ns": 120_000_000_000,
        "primary_received_unique": 129_999,
        "primary_received_events": 129_999,
        "primary_duplicates": 0,
        "primary_last_offset_ns": 119_999_500_000,
        "primary_buckets": buckets,
        "drain_coverage_start_offset_ns": 120_000_000_000,
        "drain_coverage_end_offset_ns": 130_000_000_000,
        "drain_received_unique": 1,
        "drain_received_events": 1,
        "drain_duplicates": 0,
        "drain_first_offset_ns": 120_000_500_000,
        "drain_last_offset_ns": 120_000_500_000,
        "drain_buckets": drain_buckets,
        "after_drain_unique": 0,
        "after_drain_events": 0,
        "after_drain_duplicates": 0,
        "after_drain_first_offset_ns": None,
        "after_drain_last_offset_ns": None,
        "drain_right_censored": False,
        "max_arrival_offset_ns": 120_000_500_000,
        "received_unique": 130_000,
        "received_events": 130_000,
        "duplicates": 0,
        "phase_received_messages": [55_000, 20_000, 55_000],
    }
    sequence = {"total_expected": "130000", "total_received": "130000", "gap_msgs": "0", "duplicates_count": "0"}
    return timing, source, request, sink, throughput, sequence


def test_swap4_schedule_has_30_runs_with_one_event_at_measured_t60() -> None:
    runs = build_schedule({"e-swap-4"}, seed=1729)
    assert len(runs) == 30
    assert {run.run_index for run in runs} == set(range(1, 31))
    assert all(hot_swap_offsets(run) == [60.0] for run in runs)


def test_swap4_timeline_requires_one_centered_swap_and_reconciled_phases() -> None:
    timing, source, requests, sink, throughput, sequence = swap4_fixture()
    timeline = build_swap4_timeline(timing, source, requests, sink, throughput, sequence)

    assert timeline["successful_swaps"] == 1
    assert timeline["swap_ns"] == timing["scheduled_swap_ns"] + 5_000_000
    assert timeline["swap_alignment_error_ns"] == 5_000_000
    assert timeline["phases"]["before"]["intended"] == 55_000
    assert timeline["phases"]["burst"]["received"] == 20_000
    assert timeline["sequence"] == {"expected": 130_000, "received": 130_000, "gaps": 0, "duplicates": 0}
    assert timeline["primary_received_events"] == 129_999
    assert timeline["drain_received_events"] == 1
    assert timeline["drain_duration_after_window_ns"] == 500_000
    assert timeline["source_completion_offset_ns"] == 120_001_000_000
    validate_swap4_artifacts(timeline, throughput, requests, sink)

    invalid = json.loads(json.dumps(timeline))
    invalid["successful_swaps"] = 0
    with pytest.raises(ValueError, match="exactly one"):
        validate_swap4_artifacts(invalid, throughput, requests, sink)
    invalid = json.loads(json.dumps(timeline))
    invalid["scheduled_swap_offset_ns"] += 1
    with pytest.raises(ValueError, match="centered"):
        validate_swap4_artifacts(invalid, throughput, requests, sink)
    invalid = json.loads(json.dumps(timeline))
    invalid["phases"]["burst"]["received"] -= 1
    with pytest.raises(ValueError, match="phase counts"):
        validate_swap4_artifacts(invalid, throughput, requests, sink)


def test_swap4_rejects_malformed_primary_and_drain_evidence() -> None:
    timing, source, requests, sink, throughput, sequence = swap4_fixture()
    timeline = build_swap4_timeline(timing, source, requests, sink, throughput, sequence)
    mutations = [
        ("clock", "monotonic", "source-origin clock"),
        ("source_measurement_start_unix_ns", timing["measurement_start_ns"] + 1, "source-origin clock"),
        ("drain_first_offset_ns", None, "drain offsets"),
        ("drain_right_censored", True, "right-censored"),
    ]
    for field, value, message in mutations:
        invalid = json.loads(json.dumps(throughput))
        invalid[field] = value
        with pytest.raises(ValueError, match=message):
            validate_swap4_artifacts(timeline, invalid, requests, sink)
    invalid = json.loads(json.dumps(throughput))
    invalid["primary_buckets"].pop()
    with pytest.raises(ValueError, match="1200 contiguous primary"):
        validate_swap4_artifacts(timeline, invalid, requests, sink)
    invalid = json.loads(json.dumps(throughput))
    invalid["drain_buckets"][1]["start_offset_ns"] += 1
    with pytest.raises(ValueError, match="drain buckets"):
        validate_swap4_artifacts(timeline, invalid, requests, sink)
    invalid = json.loads(json.dumps(throughput))
    invalid["max_arrival_offset_ns"] = invalid["primary_last_offset_ns"]
    with pytest.raises(ValueError, match="maximum arrival"):
        validate_swap4_artifacts(timeline, invalid, requests, sink)
    invalid_timeline = json.loads(json.dumps(timeline))
    invalid_timeline["source_completion_offset_ns"] = 130_000_000_000
    with pytest.raises(ValueError, match="completion"):
        validate_swap4_artifacts(invalid_timeline, throughput, requests, sink)


def test_swap4_summary_uses_one_event_from_each_of_30_runs() -> None:
    runs = [
        {
            "run_index": index,
            "burst_timeline": {
                "successful_swaps": 1,
                "sequence": {"gaps": 0, "duplicates": 0},
                "drain_received_events": 1 if index % 2 else 0,
                "drain_last_offset_ns": 120_000_000_000 + index if index % 2 else None,
                "drain_right_censored": False,
            },
            "hotswap_analysis": {"sample_count": 1, "events": [{"sink_observed_output_gap_ns": index * 1_000_000}]},
        }
        for index in range(1, 31)
    ]
    summary = summarize_swap4_runs(runs)
    assert summary["n_runs"] == 30
    assert summary["n_events"] == 30
    assert summary["p95_sink_observed_output_gap_ns"] == 29_000_000
    assert len(summary["bootstrap_median_ci95_ns"]) == 2
    assert summary["runs_with_drain_arrivals"] == 15
    assert summary["max_drain_arrival_offset_ns"] == 120_000_000_029

    runs[0]["hotswap_analysis"]["events"].append({"sink_observed_output_gap_ns": 1})
    with pytest.raises(ValueError, match="one event"):
        summarize_swap4_runs(runs)


def test_backpressure_requires_observed_queue_pressure_and_recovery() -> None:
    samples = [
        {"elapsed_ns": 0, "queue": "source->slow", "depth": 0, "capacity": 64, "accepted": 0, "processed": 0},
        {"elapsed_ns": 100_000_000, "queue": "source->slow", "depth": 60, "capacity": 64, "accepted": 100, "processed": 40},
        {"elapsed_ns": 200_000_000, "queue": "source->slow", "depth": 64, "capacity": 64, "accepted": 140, "processed": 76},
        {"elapsed_ns": 300_000_000, "queue": "source->slow", "depth": 4, "capacity": 64, "accepted": 140, "processed": 136},
        {"elapsed_ns": 400_000_000, "queue": "source->slow", "depth": 0, "capacity": 64, "accepted": 140, "processed": 140},
    ]

    result = analyze_backpressure(
        samples,
        offered_messages=200,
        offered_duration_ns=200_000_000,
        occupancy_threshold=0.8,
        recovery_threshold=0.1,
    )

    assert result["classification"] == "saturated-and-drained"
    assert result["threshold_crossed"] is True
    assert result["recovered"] is True
    assert result["rates_msg_s"] == {
        "offered": 1000.0,
        "accepted": 350.0,
        "processed": 350.0,
        "drained": 600.0,
    }


def test_backpressure_postprocess_writes_lossless_memory_bounded_summary(tmp_path: Path) -> None:
    (tmp_path / "metadata.json").write_text("{}")
    (tmp_path / "queue-depth.csv").write_text(
        "elapsed_ns,queue,depth,capacity,accepted,dequeued,processed\n"
        "0,slow,0,64,0,0,0\n"
        "100000000,slow,64,64,500,436,435\n"
        "1000000000,slow,0,64,1000,1000,1000\n"
    )
    (tmp_path / "memory.csv").write_text(
        "elapsed_ms,rss_bytes\n0,100000000\n1000,110000000\n"
    )
    (tmp_path / "sequence.csv").write_text(
        "total_expected,total_received,gap_ranges,gap_msgs,duplicates_count\n"
        "1000,1000,0,0,0\n"
    )
    item = RunItem(
        experiment="e-backpressure",
        condition="saturated-slow-consumer",
        run_index=1,
        config="eval/configs/e-backpressure/pipeline-saturated.toml",
        warmup_secs=0,
        measurement_secs=10,
        total_messages=1000,
    )

    postprocess_run(ROOT, item, tmp_path)

    result = json.loads((tmp_path / "backpressure.json").read_text())
    assert result["classification"] == "saturated-and-drained"
    assert result["sequence"]["lossless"] is True
    assert result["memory"]["within_limit"] is True
    assert set(result["rates_msg_s"]) == {"offered", "accepted", "processed", "drained"}


def test_backpressure_does_not_infer_saturation_from_offered_rate() -> None:
    samples = [
        {"elapsed_ns": 0, "queue": "source->sink", "depth": 0, "capacity": 64, "accepted": 0, "processed": 0},
        {"elapsed_ns": 100_000_000, "queue": "source->sink", "depth": 0, "capacity": 64, "accepted": 100, "processed": 100},
    ]

    result = analyze_backpressure(
        samples,
        offered_messages=10_000,
        offered_duration_ns=100_000_000,
        occupancy_threshold=0.8,
        recovery_threshold=0.1,
    )

    assert result["classification"] == "not-saturated"
    assert result["threshold_crossed"] is False
    assert result["rates_msg_s"]["offered"] == 100_000.0
    assert result["rates_msg_s"]["accepted"] == 1000.0
    with pytest.raises(ValueError, match="did not cross"):
        validate_backpressure_result(result)


def _startup_artifact() -> dict:
    return {
        "schema_version": 1,
        "clock": "monotonic",
        "cache_state": "warm",
        "cache_preparation": {
            "action": "none",
            "completed_before_timing": True,
        },
        "compiled_component_cache": {
            "mode": "disabled",
            "hit": False,
            "artifact": None,
            "identity": None,
        },
        "plugin_sha256": {
            "t1": "a" * 64,
        },
        "processed_messages": 1,
        "phases_ns": {
            "process_config": 10,
            "component_load_compile": 20,
            "instantiation": 30,
            "pipeline_setup": 5,
            "first_process": 35,
        },
        "total_wall_duration_ns": 105,
        "harness_overhead_tolerance_ns": 5_000_000,
    }


def test_startup_artifact_accepts_explicit_non_overlapping_phases() -> None:
    artifact = _startup_artifact()
    validate_startup_artifact(artifact)


def test_startup_artifact_rejects_missing_or_impossible_phases() -> None:
    missing = _startup_artifact()
    del missing["phases_ns"]["instantiation"]
    with pytest.raises(ValueError, match="missing startup phase"):
        validate_startup_artifact(missing)

    impossible = _startup_artifact()
    impossible["total_wall_duration_ns"] = 90
    with pytest.raises(ValueError, match="exceed total wall duration"):
        validate_startup_artifact(impossible)


def test_startup_artifact_rejects_unproven_cache_hit() -> None:
    artifact = _startup_artifact()
    artifact["compiled_component_cache"]["hit"] = True
    with pytest.raises(ValueError, match="cache hit requires"):
        validate_startup_artifact(artifact)


def test_startup_postprocessing_preserves_runtime_measurement() -> None:
    with tempfile.TemporaryDirectory() as tmp:
        output = Path(tmp)
        artifact = _startup_artifact()
        (output / "metadata.json").write_text("{}")
        (output / "startup.json").write_text(json.dumps(artifact))
        item = RunItem(
            experiment="e-perf-9",
            condition="small-warm",
            run_index=1,
            config="eval/configs/e-perf-9/pipeline-tier-small.toml",
            warmup_secs=0,
            measurement_secs=0,
            startup_mode="warm",
        )

        postprocess_run(ROOT, item, output)

        assert json.loads((output / "startup.json").read_text()) == artifact


def test_startup_postprocessing_rejects_condition_mismatch() -> None:
    with tempfile.TemporaryDirectory() as tmp:
        output = Path(tmp)
        artifact = _startup_artifact()
        artifact["cache_state"] = "cold"
        artifact["cache_preparation"]["action"] = "drop-linux-page-cache"
        (output / "metadata.json").write_text("{}")
        (output / "startup.json").write_text(json.dumps(artifact))
        item = RunItem(
            experiment="e-perf-9",
            condition="small-warm",
            run_index=1,
            config="eval/configs/e-perf-9/pipeline-tier-small.toml",
            warmup_secs=0,
            measurement_secs=0,
            startup_mode="warm",
        )

        with pytest.raises(ValueError, match="does not match condition"):
            postprocess_run(ROOT, item, output)


def capacity_scout_fixture() -> dict:
    return {
        "schema_version": 1,
        "batch_class": "capacity-scout",
        "thesis_evidence": False,
        "system": "wafer",
        "rate_msg_s": 4000,
        "source_git_sha": "a" * 40,
        "source_dirty": False,
        "measurement_duration_ns": 1_000_000_000,
        "messages": {
            "intended": 4000,
            "rejected": 10,
            "enqueued": 3990,
            "received_events": 3985,
            "received_unique": 3980,
            "downstream_lost": 10,
            "total_undelivered": 20,
            "duplicates": 5,
            "unexpected": 0,
            "ignored_warmup": 0,
        },
        "rates_msg_s": {"intended": 4000.0, "achieved": 3980.0},
        "loss_percent": 0.5,
        "latency_hdr": {"path": "latency.hdr", "sha256": "1" * 64, "samples": 3985},
        "resources": {"scope": "sut", "cpu_percent": 42.0, "max_rss_bytes": 32_000_000},
        "thermal": {"max_temperature_millicelsius": 65000, "throttled": False},
        "process_audit": {"path": "process-audit.json", "sha256": "2" * 64},
        "config": {"path": "config.toml", "sha256": "3" * 64},
        "loadgen_profile": {"path": "canonical-rate-sweep.toml", "sha256": "4" * 64},
        "provenance": {"path": "metadata.json", "sha256": "5" * 64},
        "controlled_factors": {
            "broker": "127.0.0.1:1883",
            "topic": "wafer/telemetry",
            "payload_template_sha256": "4" * 64,
            "qos": 1,
            "warmup_secs": 30,
            "measurement_secs": 60,
            "load_shape": "steady",
            "sequence_example_limit": 1_024,
            "support_cpus": "0",
            "sut_cpus": "1-3",
        },
        "traces": False,
    }


def test_capacity_scout_summary_reconciles_without_per_message_traces() -> None:
    publisher = {"intended": 4000, "rejected": 10, "enqueued": 3990}
    subscriber = {
        "total_recorded": 3985,
        "parse_errors": 0,
        "negative_latency_count": 0,
        "ignored_sequence_count": 0,
        "unexpected_sequence_count": 0,
        "latency_p50_ns": 100,
        "latency_p95_ns": 200,
        "latency_p99_ns": 300,
        "sequence": {"total_received": 3985, "total_duplicates": 5},
    }
    summary = analyze_capacity_scout_summary(publisher, subscriber, 1_000_000_000)
    assert summary["messages"] == capacity_scout_fixture()["messages"]
    assert summary["rates_msg_s"] == {"intended": 4000.0, "achieved": 3980.0}


def test_capacity_scout_result_is_emitted_from_bounded_artifacts() -> None:
    with tempfile.TemporaryDirectory() as tmp:
        output = Path(tmp)
        (output / "publisher-summary.json").write_text(json.dumps({"intended": 4000, "rejected": 10, "enqueued": 3990}))
        (output / "subscriber-metadata.json").write_text(json.dumps({
            "total_recorded": 3985,
            "total_messages": 3985,
            "parse_errors": 0,
            "negative_latency_count": 0,
            "ignored_sequence_count": 0,
            "unexpected_sequence_count": 0,
            "latency_p50_ns": 100,
            "latency_p95_ns": 200,
            "latency_p99_ns": 300,
            "histogram_lowest_ns": 1_000,
            "histogram_highest_ns": 10_000_000_000,
            "histogram_sig_digits": 3,
            "sequence": {"total_received": 3985, "total_duplicates": 5},
        }))
        (output / "latency.hdr").write_bytes(b"hdr")
        (output / "resource-usage.csv").write_text(
            "timestamp_ns,cpu_time_ticks,rss_bytes,process_count\n"
            "1000000000,100,10000000,1\n2000000000,150,20000000,1\n"
        )
        (output / "pi-telemetry.csv").write_text(
            "timestamp_ns,temperature_millicelsius,cpu_frequency_hz,governor,throttled,rail_proxy_watts\n"
            "1,64000,2400000000,performance,0x0,4.2\n"
        )
        (output / "process-audit.json").write_text("{}\n")
        (output / "config.toml").write_text("[pipeline]\n")
        (output / "loadgen-profile.toml").write_text("[loadgen]\n")
        (output / "metadata.json").write_text(
            json.dumps({"git_sha": "a" * 40, "git_dirty": False}) + "\n"
        )
        item = build_capacity_scout_rate_block(4000)[0]
        result = write_capacity_scout_result(
            ROOT,
            item,
            output,
            1_000_000_000,
            build_capacity_scout_invocation(ROOT, item.system, 4000, item.run_index, output)["controlled_factors"],
        )
        assert (output / "capacity-scout.json").is_file()
        assert result["resources"]["max_rss_bytes"] == 20_000_000
        assert result["thermal"] == {"max_temperature_millicelsius": 64000, "throttled": False}
        assert not (output / "published.csv").exists()
        assert not (output / "received.csv").exists()
        verify_capacity_scout_result_files(output, result)
        (output / "config.toml").write_text("tampered\n")
        with pytest.raises(ValueError, match="config receipt checksum mismatch"):
            verify_capacity_scout_result_files(output, result)


def test_final_capacity_result_reuses_scout_capture_with_final_semantics() -> None:
    with tempfile.TemporaryDirectory() as tmp:
        output = Path(tmp)
        (output / "publisher-summary.json").write_text(
            json.dumps(
                {
                    "schema_version": 1,
                    "intended": 240_000,
                    "rejected": 10,
                    "enqueued": 239_990,
                    "measurement_duration_ns": 60_000_000_000,
                    "deadline_misses": 12,
                }
            )
        )
        (output / "subscriber-metadata.json").write_text(
            json.dumps(
                {
                    "total_recorded": 239_985,
                    "total_messages": 239_985,
                    "parse_errors": 0,
                    "negative_latency_count": 0,
                    "ignored_sequence_count": 0,
                    "unexpected_sequence_count": 0,
                    "latency_p50_ns": 100,
                    "latency_p95_ns": 200,
                    "latency_p99_ns": 300,
                    "histogram_lowest_ns": 1_000,
                    "histogram_highest_ns": 10_000_000_000,
                    "histogram_sig_digits": 3,
                    "sequence": {"total_received": 239_985, "total_duplicates": 5},
                }
            )
        )
        for name, contents in {
            "latency.hdr": "hdr",
            "process-audit.json": "{}\n",
            "config.toml": "[pipeline]\n",
            "loadgen-profile.toml": "[loadgen]\n",
            "metadata.json": json.dumps({"git_sha": "a" * 40, "git_dirty": False}),
        }.items():
            (output / name).write_text(contents)
        (output / "resource-usage.csv").write_text(
            "timestamp_ns,cpu_time_ticks,rss_bytes,process_count\n"
            "1000000000,100,10000000,1\n2000000000,150,20000000,1\n"
        )
        (output / "pi-telemetry.csv").write_text(
            "timestamp_ns,temperature_millicelsius,cpu_frequency_hz,governor,throttled,rail_proxy_watts\n"
            "1,64000,2400000000,performance,0x0,4.2\n"
        )
        item = next(
            item
            for item in build_schedule({"e-perf-10"}, seed=1729)
            if item.system == "wafer" and item.offered_rate_msg_s == 4000
        )
        controlled = build_capacity_scout_invocation(
            ROOT, item.system, 4000, 1, output
        )["controlled_factors"]
        result = write_capacity_result(item, output, controlled)

        assert result["batch_class"] == "final-capacity"
        assert result["thesis_evidence"] is True
        assert result["messages"] == {
            "intended": 240_000,
            "rejected": 10,
            "enqueued": 239_990,
            "received_events": 239_985,
            "received_unique": 239_980,
            "downstream_lost": 10,
            "total_undelivered": 20,
            "duplicates": 5,
            "unexpected": 0,
            "ignored_warmup": 0,
        }
        assert result["rates_msg_s"]["achieved_ratio"] == pytest.approx(0.9999166667)
        assert result["latency_hdr"]["significant_digits"] == 3
        assert (output / "capacity-run.json").is_file()
        assert not (output / "published.csv").exists()
        assert not (output / "received.csv").exists()
        validate_capacity_run_result(result)
        verify_capacity_result_files(output, result)

        invalid = json.loads(json.dumps(result))
        invalid["messages"]["downstream_lost"] += 1
        with pytest.raises(ValueError, match="message counters do not reconcile"):
            validate_capacity_run_result(invalid)

        (output / "config.toml").write_text("tampered\n")
        with pytest.raises(ValueError, match="config receipt checksum mismatch"):
            verify_capacity_result_files(output, result)


def test_final_capacity_validator_rejects_wrong_duration_and_population() -> None:
    result = capacity_run_fixture("wafer", 1000)
    result["measurement_duration_ns"] = 30_000_000_000
    with pytest.raises(ValueError, match="measurement duration differs"):
        validate_capacity_run_result(result)

    result = capacity_run_fixture("wafer", 1000)
    result["messages"].update(
        intended=999,
        rejected=0,
        enqueued=999,
        received_events=999,
        received_unique=999,
    )
    result["rates_msg_s"].update(intended=16.65, achieved=16.65, achieved_ratio=0.01665)
    result["latency_hdr"]["samples"] = 999
    with pytest.raises(ValueError, match="intended population differs"):
        validate_capacity_run_result(result)


def test_capacity_scout_validator_rejects_counter_drift_and_evidence_promotion() -> None:
    result = capacity_scout_fixture()
    validate_capacity_scout_result(result)
    invalid = json.loads(json.dumps(result))
    invalid["messages"]["enqueued"] += 1
    with pytest.raises(ValueError, match=r"intended = rejected \+ enqueued"):
        validate_capacity_scout_result(invalid)
    canonical = json.loads(json.dumps(result))
    canonical["thesis_evidence"] = True
    with pytest.raises(ValueError, match="thesis_evidence=false"):
        validate_capacity_scout_result(canonical)
    traced = json.loads(json.dumps(result))
    traced["traces"] = True
    with pytest.raises(ValueError, match="must not contain per-message traces"):
        validate_capacity_scout_result(traced)
    unexpected = json.loads(json.dumps(result))
    unexpected["messages"]["unexpected"] = 1
    with pytest.raises(ValueError, match="unexpected"):
        validate_capacity_scout_result(unexpected)


def test_capacity_scout_probe_requires_three_good_runs() -> None:
    good = capacity_scout_fixture()
    assert classify_capacity_scout_probe([good, good, good]) == "good"
    bad = json.loads(json.dumps(good))
    bad["messages"].update({
        "received_events": 3905,
        "received_unique": 3900,
        "downstream_lost": 90,
        "total_undelivered": 100,
    })
    bad["rates_msg_s"]["achieved"] = 3900.0
    bad["loss_percent"] = 2.5
    bad["latency_hdr"]["samples"] = 3905
    assert classify_capacity_scout_probe([good, bad, good]) == "bad"
    with pytest.raises(ValueError, match="exactly three"):
        classify_capacity_scout_probe([good, good])


def scout_result(system: str, rate: int, classification: str = "good") -> dict:
    result = capacity_scout_fixture()
    result["system"] = system
    result["rate_msg_s"] = rate
    result["resources"]["scope"] = "no-sut" if system == "mqtt-loopback" else "sut"
    intended = rate
    rejected = 0 if classification == "good" else max(1, rate // 50)
    enqueued = intended - rejected
    result["messages"].update({
        "intended": intended,
        "rejected": rejected,
        "enqueued": enqueued,
        "received_events": enqueued,
        "received_unique": enqueued,
        "downstream_lost": 0,
        "total_undelivered": rejected,
        "duplicates": 0,
        "unexpected": 0,
        "ignored_warmup": 0,
    })
    result["rates_msg_s"] = {"intended": float(intended), "achieved": float(enqueued)}
    result["loss_percent"] = 100.0 * rejected / intended
    result["latency_hdr"]["samples"] = enqueued
    return result


def accept_capacity_scout_decision(
    decisions: list[dict], accepted: dict[str, dict], decision: dict, classifications: dict[str, str]
) -> None:
    decisions.append(decision)
    for item in decision["schedule"]:
        key = f"{item['experiment']}/{item['condition']}/run-{item['run_index']:02d}"
        accepted[key] = scout_result(
            item["system"], item["offered_rate_msg_s"], classifications[item["system"]]
        )


def test_capacity_scout_cli_derives_initial_decision_without_rate_override() -> None:
    command = [
        sys.executable,
        str(ROOT / "eval/scripts/lib/canonical_runner.py"),
        "--capacity-scout",
        "--batch-id",
        "test-dry-run",
        "--dry-run",
    ]
    result = subprocess.run(command, cwd=ROOT, text=True, capture_output=True, check=True)
    assert '"rate_msg_s": 500' in result.stdout
    assert '"kind": "geometric"' in result.stdout
    assert "--capacity-scout-rate" not in result.stdout


def test_capacity_scout_replay_starts_with_full_500_block_and_resumes_incomplete() -> None:
    first = replay_capacity_scout_decisions([], {})
    assert first["action"] == "launch"
    assert first["decision"]["kind"] == "geometric"
    assert first["decision"]["rate_msg_s"] == 500
    assert set(first["decision"]["systems"]) == {"mqtt-loopback", "native", "wafer", "ekuiper"}
    one_item = first["decision"]["schedule"][0]
    key = f"{one_item['experiment']}/{one_item['condition']}/run-{one_item['run_index']:02d}"
    accepted = {key: scout_result(one_item["system"], 500)}
    resumed = replay_capacity_scout_decisions([first["decision"]], accepted)
    assert resumed["action"] == "resume"
    assert key not in resumed["pending_result_keys"]
    assert len(resumed["pending_result_keys"]) == 11
    with pytest.raises(ValueError, match="lack a decision"):
        replay_capacity_scout_decisions([], accepted)


def test_capacity_scout_decision_replay_survives_json_roundtrip() -> None:
    decisions: list[dict] = []
    accepted: dict[str, dict] = {}
    first = replay_capacity_scout_decisions(decisions, accepted)["decision"]
    accept_capacity_scout_decision(
        decisions,
        accepted,
        first,
        {system: "good" for system in CAPACITY_SCOUT_SYSTEMS},
    )
    second = replay_capacity_scout_decisions(decisions, accepted)["decision"]
    accept_capacity_scout_decision(
        decisions,
        accepted,
        second,
        {system: "good" for system in CAPACITY_SCOUT_SYSTEMS},
    )

    validate_capacity_scout_decision_replay(json.loads(json.dumps(decisions)), accepted)


def test_capacity_scout_loader_rejects_tampered_decision_chain_and_schedule() -> None:
    with tempfile.TemporaryDirectory() as tmp:
        root = Path(tmp)
        decisions_dir = root / "eval/results/capacity-scout/rpi5-test/decisions"
        decisions_dir.mkdir(parents=True)
        first = replay_capacity_scout_decisions([], {})["decision"]
        first_path = decisions_dir / "decision-0001.json"
        first_path.write_text(json.dumps(first))
        second = {**first, "decision_index": 2, "previous_decision_sha256": "0" * 64}
        (decisions_dir / "decision-0002.json").write_text(json.dumps(second))
        with pytest.raises(ValueError, match="hash chain"):
            load_capacity_scout_replay(root, "test")

    with tempfile.TemporaryDirectory() as tmp:
        root = Path(tmp)
        decisions_dir = root / "eval/results/capacity-scout/rpi5-test/decisions"
        decisions_dir.mkdir(parents=True)
        first = replay_capacity_scout_decisions([], {})["decision"]
        first["schedule"][0]["offered_rate_msg_s"] = 999
        (decisions_dir / "decision-0001.json").write_text(json.dumps(first))
        with pytest.raises(ValueError, match="not reproducible"):
            load_capacity_scout_replay(root, "test")


def test_capacity_scout_loader_rejects_duplicate_passed_logical_runs() -> None:
    with tempfile.TemporaryDirectory() as tmp:
        root = Path(tmp)
        batch = root / "eval/results/capacity-scout/rpi5-test"
        decision = replay_capacity_scout_decisions([], {})["decision"]
        decisions = batch / "decisions"
        decisions.mkdir(parents=True)
        (decisions / "decision-0001.json").write_text(json.dumps(decision))
        for attempt in (1, 2):
            path = batch / "wafer/rate-00500" / f"run-01-attempt-{attempt:02d}"
            path.mkdir(parents=True)
            (path / "capacity-scout.json").write_text(json.dumps(scout_result("wafer", 500)))
            (path / "canonical-status.json").write_text(json.dumps({"status": "passed"}))
        with pytest.raises(ValueError, match="duplicate accepted"):
            load_capacity_scout_replay(root, "test")


def test_capacity_scout_replay_doubles_past_16000() -> None:
    decisions: list[dict] = []
    accepted: dict[str, dict] = {}
    outcome = replay_capacity_scout_decisions(decisions, accepted)
    for rate in (500, 1000, 2000, 4000, 8000, 16000):
        assert outcome["decision"]["rate_msg_s"] == rate
        accept_capacity_scout_decision(
            decisions, accepted, outcome["decision"],
            {system: "good" for system in CAPACITY_SCOUT_SYSTEMS},
        )
        outcome = replay_capacity_scout_decisions(decisions, accepted)
    assert outcome["decision"]["kind"] == "geometric"
    assert outcome["decision"]["rate_msg_s"] == 32000


def test_capacity_scout_replay_mqtt_bad_confirmation_censors_suts() -> None:
    decisions: list[dict] = []
    accepted: dict[str, dict] = {}
    first = replay_capacity_scout_decisions(decisions, accepted)
    accept_capacity_scout_decision(
        decisions, accepted, first["decision"],
        {system: "good" for system in CAPACITY_SCOUT_SYSTEMS},
    )
    second = replay_capacity_scout_decisions(decisions, accepted)
    accept_capacity_scout_decision(
        decisions, accepted, second["decision"],
        {"mqtt-loopback": "bad", "native": "good", "wafer": "good", "ekuiper": "good"},
    )
    confirmation = replay_capacity_scout_decisions(decisions, accepted)
    assert confirmation["decision"]["kind"] == "mqtt-confirmation"
    assert confirmation["decision"]["systems"] == ["mqtt-loopback"]
    classified = confirmation["decision"]["state_before"]["classifications"][-1]["results"]
    assert classified["mqtt-loopback"] == "bad"
    assert all(classified[system] == "support-confounded" for system in ("native", "wafer", "ekuiper"))
    accept_capacity_scout_decision(
        decisions, accepted, confirmation["decision"], {"mqtt-loopback": "bad"}
    )
    after_confirmation = replay_capacity_scout_decisions(decisions, accepted)
    assert after_confirmation["action"] == "stop"
    states = after_confirmation["states"]
    assert all(states[system] == {"phase": "support-censored", "censor_above_rate_msg_s": 500} for system in ("native", "wafer", "ekuiper"))
    assert states["mqtt-loopback"] == {
        "phase": "resolved",
        "lower_good_rate_msg_s": 500,
        "upper_bad_rate_msg_s": 1000,
        "support_censor_above_rate_msg_s": 500,
    }


def test_capacity_scout_replay_refines_each_sut_then_stops() -> None:
    decisions: list[dict] = []
    accepted: dict[str, dict] = {}
    outcome = replay_capacity_scout_decisions(decisions, accepted)
    for rate, sut_classification in ((500, "good"), (1000, "good"), (2000, "bad"), (4000, "bad")):
        assert outcome["decision"]["rate_msg_s"] == rate
        classifications = {"mqtt-loopback": "good"} | {
            system: sut_classification for system in ("native", "wafer", "ekuiper")
        }
        accept_capacity_scout_decision(decisions, accepted, outcome["decision"], classifications)
        outcome = replay_capacity_scout_decisions(decisions, accepted)
    for system in ("native", "wafer", "ekuiper"):
        assert outcome["decision"]["kind"] == "refinement"
        assert outcome["decision"]["systems"] == [system]
        assert outcome["decision"]["rate_msg_s"] == 1500
        accept_capacity_scout_decision(
            decisions, accepted, outcome["decision"], {system: "bad"}
        )
        outcome = replay_capacity_scout_decisions(decisions, accepted)
    assert outcome["action"] == "stop"
    assert outcome["reason"] == "all-suts-resolved-or-support-censored"


def test_capacity_scout_invalid_counter_attempt_is_a_hard_stop() -> None:
    with tempfile.TemporaryDirectory() as tmp:
        output = Path(tmp)
        (output / "canonical-status.json").write_text(
            json.dumps({"status": "failed", "detail": "capacity-scout message counter differs: enqueued"})
        )
        assert capacity_scout_failed_attempt_stop_reason(output) == "provenance-or-counter-drift"


def test_capacity_scout_progress_has_one_complete_schema() -> None:
    with tempfile.TemporaryDirectory() as tmp:
        ledger = Path(tmp)
        item = build_capacity_scout_rate_block(500)[0]
        counters = capacity_scout_fixture()["messages"]
        write_capacity_scout_progress(
            ledger, "probe-finished", item, 2, 64_000, False, counters, None
        )
        entry = json.loads((ledger / "progress.jsonl").read_text())
    assert set(entry) == {
        "timestamp", "event", "probe", "condition", "run", "attempt",
        "temperature_millicelsius", "throttled", "counters", "error",
    }
    assert entry["attempt"] == 2
    assert entry["temperature_millicelsius"] == 64_000
    assert entry["counters"] == counters


def test_capacity_scout_source_state_uses_deploy_receipt_without_git_checkout() -> None:
    with tempfile.TemporaryDirectory() as tmp:
        root = Path(tmp)
        receipt = {
            "git_sha": "a" * 40,
            "git_dirty": False,
            "git_tags": ["rpi5-capacity-scout-v2"],
        }
        (root / "SOURCE_STATE.json").write_text(json.dumps(receipt))

        assert capacity_scout_source_state(root) == receipt


def test_capacity_scout_safety_boundaries() -> None:
    safe = {
        "provenance_matches": True,
        "telemetry_available": True,
        "throttled": False,
        "temperature_millicelsius": 65_000,
        "attempt_elapsed_secs": 1,
        "elapsed_secs": 1,
        "free_bytes": 4 * 1024**3,
        "largest_probe_bytes": 1024**3,
        "repeated_systemic_failures": 0,
    }
    assert capacity_scout_safety_action(safe) == {"action": "proceed"}
    for field, value, reason in (
        ("provenance_matches", False, "provenance-drift"),
        ("telemetry_available", False, "telemetry-unavailable"),
        ("throttled", True, "throttling"),
        ("temperature_millicelsius", 75_000, "temperature-75c"),
        ("attempt_elapsed_secs", 240, "attempt-240s"),
        ("elapsed_secs", 18 * 60 * 60, "batch-18h"),
        ("free_bytes", 2 * 1024**3 - 1, "disk-floor"),
        ("repeated_systemic_failures", 3, "repeated-systemic-failure"),
    ):
        snapshot = {**safe, field: value}
        assert capacity_scout_safety_action(snapshot)["reason"] == reason
    assert capacity_scout_safety_action({**safe, "temperature_millicelsius": 70_000}) == {
        "action": "pause",
        "reason": "temperature-cool-below-65c",
    }


def test_capacity_scout_state_machine_doubles_refines_and_censors() -> None:
    history = [(500, "good"), (1000, "good"), (2000, "good"), (4000, "bad"), (8000, "bad")]
    assert capacity_scout_next_rate(history) == {"phase": "refine", "rate_msg_s": 3000}
    assert capacity_scout_next_rate(history + [(3000, "bad")]) == {"phase": "refine", "rate_msg_s": 2500}
    assert capacity_scout_next_rate(history + [(3000, "bad"), (2500, "good")]) == {
        "phase": "resolved",
        "lower_good_rate_msg_s": 2500,
        "upper_bad_rate_msg_s": 3000,
    }
    assert capacity_scout_next_rate([(rate, "good") for rate in (500, 1000, 2000, 4000, 8000, 16000)]) == {
        "phase": "geometric",
        "rate_msg_s": 32000,
    }
    assert capacity_scout_next_rate([(8000, "good"), (16000, "bad"), (32000, "bad")], mqtt=True)["support_censor_above_rate_msg_s"] == 8000


def test_capacity_scout_rate_block_is_seeded_balanced_and_positive() -> None:
    block = build_capacity_scout_rate_block(3000)
    assert len(block) == 12
    assert [item.system for item in block[:4]] == ["wafer", "mqtt-loopback", "ekuiper", "native"]
    positions = {index: [] for index in range(4)}
    for run_index in range(1, 4):
        run = [item for item in block if item.run_index == run_index]
        for position, item in enumerate(run):
            positions[position].append(item.system)
    assert all(len(set(systems)) == 3 for systems in positions.values())
    with pytest.raises(ValueError, match="positive"):
        build_capacity_scout_rate_block(0)


def test_capacity_scout_decision_is_persisted_once() -> None:
    with tempfile.TemporaryDirectory() as tmp:
        path = Path(tmp) / "decision.json"
        decision = {"phase": "geometric", "rate_msg_s": 500}
        assert persist_capacity_scout_decision(path, decision) is True
        assert path.exists()
        assert persist_capacity_scout_decision(path, decision) is False
        with pytest.raises(ValueError, match="another decision"):
            persist_capacity_scout_decision(path, {"phase": "geometric", "rate_msg_s": 1000})


def test_capacity_scout_invocations_match_controlled_factors_and_are_trace_free() -> None:
    receipts = [build_capacity_scout_invocation(ROOT, system, 3000, 2, Path("/tmp/scout")) for system in ("mqtt-loopback", "native", "wafer", "ekuiper")]
    controlled = [receipt["controlled_factors"] for receipt in receipts]
    assert all(value == controlled[0] for value in controlled[1:])
    assert {receipt["system"] for receipt in receipts} == {"mqtt-loopback", "native", "wafer", "ekuiper"}
    assert all("--trace-file" not in receipt["publisher_command"] for receipt in receipts)
    assert all("--trace-file" not in receipt["subscriber_command"] for receipt in receipts)
    assert all("--sequence-example-limit" in receipt["subscriber_command"] for receipt in receipts)
    assert all(receipt["publisher_command"][receipt["publisher_command"].index("--rate") + 1] == "3000" for receipt in receipts)
    wafer = next(receipt for receipt in receipts if receipt["system"] == "wafer")
    assert wafer["config"] == "eval/configs/capacity-scout-wafer.toml"


def test_final_schedule_contains_every_declared_condition_once_per_run() -> None:
    matrix = json.loads((ROOT / "eval/canonical-matrix.json").read_text())
    schedule = build_schedule(set(matrix["experiments"]), seed=1729)
    assert len(schedule) == matrix["final_campaign"]["expected_schedule_records"] == 2_105
    keys = [item.result_key for item in schedule]
    assert len(keys) == len(set(keys))
    for experiment, definition in matrix["experiments"].items():
        items = [item for item in schedule if item.experiment == experiment]
        expected = definition["repetitions"] * len(definition["conditions"])
        if experiment == "e-perf-10":
            expected *= len(definition["rate_points_msg_s"])
        assert len(items) == expected, experiment


def test_rate_sweep_schedule_is_complete_and_position_balanced() -> None:
    matrix = json.loads((ROOT / "eval/canonical-matrix.json").read_text())
    definition = matrix["experiments"]["e-perf-10"]
    schedule = build_schedule({"e-perf-10"}, seed=matrix["final_campaign"]["seed"])
    systems = tuple(definition["systems"])
    rates = tuple(definition["rate_points_msg_s"])

    assert rates == (1000, 4000, 8000, 15000, 16000)
    assert len(schedule) == 30 * len(systems) * len(rates) == 600
    assert {
        (item.system, item.offered_rate_msg_s)
        for item in schedule
    } == set((system, rate) for system in systems for rate in rates)
    assert all(item.exclusive_sut for item in schedule)
    assert all(item.total_messages == item.offered_rate_msg_s * 60 for item in schedule)
    assert all(item.loadgen_profile == "eval/loadgen/canonical-rate-sweep.toml" for item in schedule)
    assert schedule == build_schedule({"e-perf-10"}, seed=1729)
    assert schedule != build_schedule({"e-perf-10"}, seed=1730)

    for rate in rates:
        positions = {position: [] for position in range(len(systems))}
        for run_index in range(1, 31):
            block = [
                item
                for item in schedule
                if item.run_index == run_index and item.offered_rate_msg_s == rate
            ]
            assert len(block) == len(systems)
            for position, item in enumerate(block):
                positions[position].append(item.system)
        assert all(set(observed) == set(systems) for observed in positions.values())


def test_final_capacity_loadgen_commands_use_bounded_summaries_without_raw_traces() -> None:
    item = next(
        item
        for item in build_schedule({"e-perf-10"}, seed=1729)
        if item.system == "wafer" and item.offered_rate_msg_s == 4000
    )
    output = Path("/tmp/rate-sweep")
    publisher = loadgen_command(
        ROOT,
        item,
        "publish",
        topic="wafer/telemetry",
        summary_file=output / "publisher-summary.json",
    )
    warmup = loadgen_command(
        ROOT,
        item,
        "publish",
        duration=item.warmup_secs,
        topic="wafer/telemetry",
        sequence_start=item.total_messages,
    )
    subscriber = loadgen_command(
        ROOT,
        item,
        "subscribe",
        output=output,
        topic="wafer/telemetry/hot",
    )

    assert publisher[publisher.index("--rate") + 1] == "4000"
    assert publisher[publisher.index("--topic") + 1] == "wafer/telemetry"
    assert publisher[publisher.index("--summary-file") + 1] == str(
        output / "publisher-summary.json"
    )
    assert "--trace-file" not in publisher
    assert "--drop-when-full" in publisher
    assert warmup[warmup.index("--sequence-start") + 1] == "240000"
    assert "--drop-when-full" in warmup
    assert subscriber[subscriber.index("--total-messages") + 1] == "240000"
    assert subscriber[subscriber.index("--sequence-end-exclusive") + 1] == "240000"
    assert subscriber[subscriber.index("--topic") + 1] == "wafer/telemetry/hot"
    assert subscriber[subscriber.index("--sequence-example-limit") + 1] == "1024"
    assert "--trace-file" not in subscriber


def capacity_run_fixture(
    system: str,
    rate: int,
    *,
    run_index: int = 1,
    loss_ratio: float = 0.0,
    achieved_ratio: float = 1.0,
    p99_ns: int = 1_000_000,
) -> dict:
    intended = rate * 60
    total_undelivered = round(intended * loss_ratio)
    received_unique = round(intended * achieved_ratio)
    rejected = min(total_undelivered, intended - received_unique)
    enqueued = intended - rejected
    downstream_lost = enqueued - received_unique
    result = capacity_scout_fixture()
    result.update(
        {
            "batch_class": "final-capacity",
            "experiment": "e-perf-10",
            "thesis_evidence": True,
            "system": system,
            "rate_msg_s": rate,
            "run_index": run_index,
            "measurement_duration_ns": 60_000_000_000,
            "messages": {
                "intended": intended,
                "rejected": rejected,
                "enqueued": enqueued,
                "received_events": received_unique,
                "received_unique": received_unique,
                "downstream_lost": downstream_lost,
                "total_undelivered": total_undelivered,
                "duplicates": 0,
                "unexpected": 0,
                "ignored_warmup": 0,
            },
            "rates_msg_s": {
                "intended": float(rate),
                "achieved": float(received_unique) / 60,
                "achieved_ratio": achieved_ratio,
            },
            "loss_percent": 100.0 * loss_ratio,
            "latency_ns": {"p50": p99_ns // 2, "p95": p99_ns, "p99": p99_ns},
            "latency_hdr": {
                "path": "latency.hdr",
                "sha256": "1" * 64,
                "samples": received_unique,
                "lowest_ns": 1_000,
                "highest_ns": 10_000_000_000,
                "significant_digits": 3,
            },
            "traces": False,
        }
    )
    return result


def capacity_batch(
    classifications: dict[str, dict[int, tuple[float, float, int]]],
) -> dict[str, list[dict]]:
    return {
        system: [
            capacity_run_fixture(
                system,
                rate,
                run_index=run_index,
                loss_ratio=loss,
                achieved_ratio=achieved,
                p99_ns=p99,
            )
            for rate, (loss, achieved, p99) in rates.items()
            for run_index in range(1, 31)
        ]
        for system, rates in classifications.items()
    }


def test_capacity_estimator_rejects_duplicate_run_indices() -> None:
    rates = {
        rate: (0.0, 1.0, 1_000_000)
        for rate in (1000, 4000, 8000, 15000, 16000)
    }
    runs = capacity_batch(
        {system: rates for system in ("mqtt-loopback", "native", "wafer", "ekuiper")}
    )
    runs["wafer"][1]["run_index"] = 1
    with pytest.raises(ValueError, match="duplicate run_index"):
        estimate_capacity_envelope(runs)


def test_capacity_estimator_handles_first_rate_failure() -> None:
    runs = capacity_batch(
        {
            system: {
                1000: (0.02, 0.98, 1_000_000),
                4000: (0.02, 0.98, 1_000_000),
                8000: (0.02, 0.98, 1_000_000),
                15000: (0.02, 0.98, 1_000_000),
                16000: (0.02, 0.98, 1_000_000),
            }
            for system in ("mqtt-loopback", "native", "wafer", "ekuiper")
        }
    )
    summary = estimate_capacity_envelope(runs)
    assert summary["systems"]["mqtt-loopback"]["delivery_ceiling"] == {
        "rate_msg_s": None,
        "censoring": "left-censored-below-1000",
    }
    assert summary["systems"]["wafer"]["support_censoring"]["from_rate_msg_s"] == 1000


def test_capacity_estimator_handles_interior_failure_and_normalized_knee() -> None:
    rates = {
        1000: (0.0, 1.0, 1_000_000),
        4000: (0.0, 1.0, 1_900_000),
        8000: (0.02, 0.98, 2_100_000),
        15000: (0.02, 0.98, 3_000_000),
        16000: (0.02, 0.98, 4_000_000),
    }
    summary = estimate_capacity_envelope(
        capacity_batch({system: rates for system in ("mqtt-loopback", "native", "wafer", "ekuiper")})
    )
    mqtt = summary["systems"]["mqtt-loopback"]
    assert mqtt["delivery_ceiling"] == {"rate_msg_s": 4000, "censoring": "none"}
    assert mqtt["normalized_p99_knee"] == {"rate_msg_s": 8000, "censoring": "none"}
    assert mqtt["rates"][1]["pooled_loss"] == 0.0
    assert mqtt["rates"][1]["mean_achieved_ratio"] == 1.0
    assert mqtt["rates"][1]["run_summary"]["p99_ns"]["median"] == 1_900_000


def test_capacity_estimator_handles_all_rates_good() -> None:
    rates = {
        rate: (0.0, 1.0, 1_000_000)
        for rate in (1000, 4000, 8000, 15000, 16000)
    }
    summary = estimate_capacity_envelope(
        capacity_batch({system: rates for system in ("mqtt-loopback", "native", "wafer", "ekuiper")})
    )
    assert summary["systems"]["wafer"]["delivery_ceiling"] == {
        "rate_msg_s": 16000,
        "censoring": "right-censored-above-16000",
    }
    assert summary["systems"]["wafer"]["normalized_p99_knee"] == {
        "rate_msg_s": None,
        "censoring": "right-censored-above-16000",
    }


def test_capacity_estimator_flags_non_contiguous_good_rates() -> None:
    rates = {
        1000: (0.0, 1.0, 1_000_000),
        4000: (0.02, 0.98, 1_000_000),
        8000: (0.0, 1.0, 1_000_000),
        15000: (0.02, 0.98, 1_000_000),
        16000: (0.02, 0.98, 1_000_000),
    }
    summary = estimate_capacity_envelope(
        capacity_batch({system: rates for system in ("mqtt-loopback", "native", "wafer", "ekuiper")})
    )
    mqtt = summary["systems"]["mqtt-loopback"]
    assert mqtt["non_monotonic"] is True
    assert mqtt["delivery_ceiling"] == {"rate_msg_s": 8000, "censoring": "non-monotonic"}


def test_capacity_estimator_marks_sut_rates_support_censored() -> None:
    mqtt = {
        1000: (0.0, 1.0, 1_000_000),
        4000: (0.0, 1.0, 1_000_000),
        8000: (0.02, 0.98, 1_000_000),
        15000: (0.02, 0.98, 1_000_000),
        16000: (0.02, 0.98, 1_000_000),
    }
    sut = {
        rate: (0.0, 1.0, 1_000_000)
        for rate in (1000, 4000, 8000, 15000, 16000)
    }
    summary = estimate_capacity_envelope(
        capacity_batch(
            {
                "mqtt-loopback": mqtt,
                "native": sut,
                "wafer": sut,
                "ekuiper": sut,
            }
        )
    )
    wafer = summary["systems"]["wafer"]
    assert wafer["support_censoring"] == {
        "from_rate_msg_s": 8000,
        "highest_support_uncensored_rate_msg_s": 4000,
    }
    assert wafer["delivery_ceiling"] == {
        "rate_msg_s": 4000,
        "censoring": "right-censored-above-4000-by-support-path",
    }
    assert [rate["classification"] for rate in wafer["rates"]] == [
        "good",
        "good",
        "support-confounded",
        "support-confounded",
        "support-confounded",
    ]


def test_rate_sweep_result_schema_rejects_every_required_field() -> None:
    result = {
        "schema_version": 1,
        "experiment": "e-perf-10",
        "system": "wafer",
        "thesis_evidence": False,
        "measurement_boundary": "publisher run window to subscriber receive timestamp",
        "units": {"rate": "messages/second", "latency": "nanoseconds", "rss": "bytes"},
        "offered_rate_msg_s": 1000,
        "actual_offered_rate_msg_s": 999.0,
        "achieved_rate_msg_s": 998.5,
        "measurement_duration_ns": 60_000_000_000,
        "messages": {"offered": 60_000, "received": 59_910, "lost": 90, "duplicates": 0},
        "loss_percent": 0.15,
        "latency_ns": {"p50": 100_000, "p95": 200_000, "p99": 300_000},
        "resources": {"scope": "sut", "cpu_percent": 42.0, "max_rss_bytes": 32_000_000},
        "throttled": False,
        "profile": {
            "path": "eval/loadgen/canonical-rate-sweep.toml",
            "sha256": "1" * 64,
            "payload_template_sha256": "4" * 64,
        },
        "process_audit": {"path": "process-audit.json", "sha256": "5" * 64},
        "traces": {
            "published": {"path": "published.csv", "sha256": "2" * 64, "samples": 60_000},
            "received": {"path": "received.csv", "sha256": "3" * 64, "samples": 59_910},
        },
    }
    validate_rate_sweep_result(result)

    for field in tuple(result):
        invalid = {**result}
        invalid.pop(field)
        with pytest.raises(ValueError, match=field):
            validate_rate_sweep_result(invalid)

    for section, fields in {
        "messages": ("offered", "received", "lost", "duplicates"),
        "latency_ns": ("p50", "p95", "p99"),
        "resources": ("scope", "cpu_percent", "max_rss_bytes"),
        "profile": ("path", "sha256", "payload_template_sha256"),
        "process_audit": ("path", "sha256"),
        "traces": ("published", "received"),
    }.items():
        for field in fields:
            invalid = json.loads(json.dumps(result))
            invalid[section].pop(field)
            with pytest.raises(ValueError, match=field):
                validate_rate_sweep_result(invalid)


def test_rate_sweep_trace_analysis_preserves_pairs_and_counts_loss() -> None:
    with tempfile.TemporaryDirectory() as tmp:
        root = Path(tmp)
        published = root / "published.csv"
        received = root / "received.csv"
        metadata = root / "subscriber-metadata.json"
        published.write_text("seq,ts_ns\n0,100\n1,200\n2,300\n3,400\n")
        received.write_text(
            "seq,payload_ts_ns,receive_ns,latency_ns\n"
            "0,100,110,10\n1,200,220,20\n1,200,225,25\n"
        )
        metadata.write_text(
            json.dumps(
                {
                    "total_messages": 3,
                    "total_recorded": 3,
                    "parse_errors": 0,
                    "negative_latency_count": 0,
                    "sequence": {"total_received": 3, "total_duplicates": 1},
                    "latency_p50_ns": 20,
                    "latency_p95_ns": 25,
                    "latency_p99_ns": 25,
                }
            )
        )
        result = analyze_rate_sweep_traces(published, received, metadata)
        assert result["messages"] == {
            "offered": 4,
            "received": 2,
            "lost": 2,
            "duplicates": 1,
        }
        assert result["latency_ns"] == {"p50": 20, "p95": 25, "p99": 25}

        received.write_text(
            "seq,payload_ts_ns,receive_ns,latency_ns\n0,101,110,9\n"
        )
        with pytest.raises(ValueError, match="timestamp changed"):
            analyze_rate_sweep_traces(published, received, metadata)


def test_process_resource_sampler_records_memory_regions_and_threads() -> None:
    with tempfile.TemporaryDirectory() as tmp:
        proc = Path(tmp)
        pid = proc / "10"
        (pid / "task/10").mkdir(parents=True)
        (pid / "task/11").mkdir()
        (pid / "stat").write_text("10 (wafer) S " + " ".join(["0"] * 10 + ["5", "7"]) + "\n")
        (pid / "statm").write_text("100 3\n")
        (pid / "status").write_text(
            "VmSize:\t1000 kB\nVmRSS:\t600 kB\nRssAnon:\t400 kB\n"
            "RssFile:\t180 kB\nVmData:\t500 kB\n"
        )
        (pid / "smaps_rollup").write_text(
            "Pss_Anon:\t350 kB\nPrivate_Dirty:\t430 kB\n"
        )

        sample = ProcessResourceSampler(Path(tmp) / "out.csv", [10], proc_root=proc)._sample()

    assert sample["cpu_time_ticks"] == 12
    assert sample["process_count"] == 1
    assert sample["thread_count"] == 2
    assert sample["rss_anon_bytes"] == 400 * 1024
    assert sample["rss_file_bytes"] == 180 * 1024
    assert sample["vm_data_bytes"] == 500 * 1024
    assert sample["vm_size_bytes"] == 1000 * 1024
    assert sample["pss_anon_bytes"] == 350 * 1024
    assert sample["private_dirty_bytes"] == 430 * 1024


def test_process_resource_summary_reports_average_cpu_and_peak_rss() -> None:
    with tempfile.TemporaryDirectory() as tmp:
        path = Path(tmp) / "resource-usage.csv"
        path.write_text(
            "timestamp_ns,cpu_time_ticks,rss_bytes,process_count\n"
            "1000000000,100,10000000,1\n"
            "3000000000,300,20000000,1\n"
        )
        result = summarize_process_resources(path, clock_ticks=100)
    assert result == {
        "scope": "sut",
        "cpu_percent": 100.0,
        "max_rss_bytes": 20_000_000,
    }


def test_sustainable_throughput_classifies_last_good_and_first_bad() -> None:
    samples = [
        {"offered_rate_msg_s": 1000, "p99_ns": 1_000_000, "offered": 1000, "lost": 0},
        {"offered_rate_msg_s": 2000, "p99_ns": 1_900_000, "offered": 2000, "lost": 10},
        {"offered_rate_msg_s": 4000, "p99_ns": 2_100_000, "offered": 4000, "lost": 0},
        {"offered_rate_msg_s": 8000, "p99_ns": 1_500_000, "offered": 8000, "lost": 200},
    ]
    result = classify_sustainable_throughput(samples)
    assert result["baseline_p99_ns"] == 1_000_000
    assert result["last_good_rate_msg_s"] == 2000
    assert result["first_bad_rate_msg_s"] == 4000
    assert result["no_saturation_within_range"] is False
    assert result["rates"][2]["breaches"] == ["p99"]
    assert result["rates"][3]["breaches"] == ["loss"]


def test_sustainable_throughput_reports_no_saturation_within_range() -> None:
    samples = [
        {"offered_rate_msg_s": rate, "p99_ns": p99, "offered": rate, "lost": 0}
        for rate, p99 in ((1000, 1_000_000), (2000, 1_500_000), (4000, 2_000_000))
    ]
    result = classify_sustainable_throughput(samples)
    assert result["last_good_rate_msg_s"] == 4000
    assert result["first_bad_rate_msg_s"] is None
    assert result["highest_tested_rate_msg_s"] == 4000
    assert result["no_saturation_within_range"] is True


def test_rate_sweep_summary_uses_only_passed_attempts() -> None:
    with tempfile.TemporaryDirectory() as tmp:
        root = Path(tmp)
        ledger = root / "eval/results/canonical-batches/rpi5-test"
        ledger.mkdir(parents=True)
        matrix = root / "eval/canonical-matrix.json"
        matrix.parent.mkdir(parents=True, exist_ok=True)
        matrix.write_text((ROOT / "eval/canonical-matrix.json").read_text())
        for rate, p99, status in ((1000, 1_000_000, "passed"), (2000, 3_000_000, "passed"), (4000, 500_000, "failed")):
            run = root / f"eval/results/e-perf-10/rpi5-test/wafer/rate-{rate:05d}/run-01"
            run.mkdir(parents=True)
            result = {
                "schema_version": 1,
                "experiment": "e-perf-10",
                "system": "wafer",
                "thesis_evidence": False,
                "measurement_boundary": "publisher run window to subscriber receive timestamp",
                "units": {"rate": "messages/second", "latency": "nanoseconds", "rss": "bytes"},
                "offered_rate_msg_s": rate,
                "actual_offered_rate_msg_s": float(rate),
                "achieved_rate_msg_s": float(rate),
                "measurement_duration_ns": 1_000_000_000,
                "messages": {"offered": rate, "received": rate, "lost": 0, "duplicates": 0},
                "loss_percent": 0.0,
                "latency_ns": {"p50": p99, "p95": p99, "p99": p99},
                "resources": {"scope": "sut", "cpu_percent": 1.0, "max_rss_bytes": 1},
                "throttled": False,
                "profile": {
                    "path": "profile.toml",
                    "sha256": "1" * 64,
                    "payload_template_sha256": "4" * 64,
                },
                "process_audit": {"path": "process-audit.json", "sha256": "5" * 64},
                "traces": {
                    "published": {"path": "published.csv", "sha256": "2" * 64, "samples": rate},
                    "received": {"path": "received.csv", "sha256": "3" * 64, "samples": rate},
                },
            }
            (run / "rate-sweep.json").write_text(json.dumps(result))
            (run / "canonical-status.json").write_text(json.dumps({"status": status}))
        summary_path = summarize_rate_sweep(root, "test")
        summary = json.loads(summary_path.read_text())
    wafer = summary["systems"]["wafer"]
    assert wafer["first_bad_rate_msg_s"] == 2000
    assert "4000" not in wafer["observed_samples"]
    assert summary["thesis_evidence"] is False


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
    assert {item.condition for item in by_experiment["e-iso-7"]} == {
        "control",
        "panic-attack",
        "epoch-loop-attack",
    }
    assert len(by_experiment["e-swap-1"]) == 1
    assert len(by_experiment["e-swap-2"]) == 1
    assert len(by_experiment["e-swap-4"]) == 30
    assert all(item.events_per_run is None for item in by_experiment["e-swap-4"])
    assert len(by_experiment["e-swap-5"]) == 1
    assert len(by_experiment["e-swap-6"]) == 1
    assert by_experiment["e-swap-2"][0].shared_from == "e-swap-1"
    assert by_experiment["e-swap-6"][0].shared_from == "e-swap-1"
    assert all("shakedown-macos" not in item.result_key for item in schedule)

    matrix = json.loads((ROOT / "eval/canonical-matrix.json").read_text())["experiments"]
    for index in range(1, 9):
        assert "per_node_metrics.csv" in matrix[f"e-iso-{index}"]["required_outputs"]
    iso_7_outputs = set(matrix["e-iso-7"]["required_outputs"])
    assert "branch-isolation.json" in iso_7_outputs
    assert not {"latency.hdr", "throughput.csv", "sequence.csv"} & iso_7_outputs
    assert {"recovery.csv", "recovery.json"} <= set(matrix["e-iso-8"]["required_outputs"])
    for index in range(1, 7):
        assert "swap_timeline.json" in matrix[f"e-swap-{index}"]["required_outputs"]
    for index in (1, 2, 4, 6):
        outputs = set(matrix[f"e-swap-{index}"]["required_outputs"])
        assert {"swap_requests.json", "hotswap-analysis.json"} <= outputs
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


def test_branch_isolation_uses_branch_artifacts_not_aggregate_fan_in() -> None:
    with tempfile.TemporaryDirectory() as tmp:
        root = Path(tmp)
        (root / "throughput.csv").write_text("elapsed_secs,msg_count,bytes\n1.0,999999,1\n")
        (root / "sequence.csv").write_text(
            "total_expected,total_received,gap_ranges,gap_msgs,duplicates_count\n"
            "180000,180000,0,0,90000\n"
        )
        (root / "percentiles.json").write_text(
            json.dumps({"total_count": 180000, "p50_ns": 1, "p95_ns": 1, "p99_ns": 1, "p999_ns": 1})
        )

        for branch, received, p95 in (("branch-a", 60000, 250000), ("branch-b", 0, 0)):
            branch_dir = root / branch
            branch_dir.mkdir()
            branch_dir.joinpath("throughput.csv").write_text(
                "elapsed_secs,msg_count,bytes\n30.0,30000,3840000\n60.0,30000,3840000\n"
                if received
                else "elapsed_secs,msg_count,bytes\n"
            )
            branch_dir.joinpath("sequence.csv").write_text(
                "total_expected,total_received,gap_ranges,gap_msgs,duplicates_count\n"
                f"{received},{received},0,0,0\n"
            )
            branch_dir.joinpath("percentiles.json").write_text(
                json.dumps(
                    {
                        "total_count": received,
                        "p50_ns": p95 // 2,
                        "p95_ns": p95,
                        "p99_ns": p95 + 10000 if received else 0,
                        "p999_ns": p95 + 20000 if received else 0,
                    }
                )
            )
            if received:
                branch_dir.joinpath("measurement-window.json").write_text(
                    json.dumps({"started_ns": 1_000_000_000, "finished_ns": 61_000_000_000})
                )

        summary = derive_branch_isolation(
            root,
            warmup_secs=30,
            measurement_secs=60,
            branch_sources={"branch-a": "source_a", "branch-b": "source_b"},
            target_messages=60_000,
        )
        branch_a = summary["branches"]["branch_a"]
        assert branch_a["source_node"] == "source_a"
        assert branch_a["sequence_scope"] == "post_warmup"
        assert branch_a["target_messages"] == 60_000
        assert branch_a["offered_messages"] == 60000
        assert branch_a["received_messages"] == 60000
        assert branch_a["gap_messages"] == 0
        assert branch_a["duplicates"] == 0
        assert branch_a["throughput"]["total_messages"] == 60000
        assert branch_a["throughput"]["samples"][0]["msg_count"] == 30000
        assert branch_a["latency_ns"]["p95"] == 250000
        assert summary["branches"]["branch_b"]["received_messages"] == 0
        assert summary["measurement_boundary"]["warmup_secs"] == 30
        assert summary["measurement_boundary"]["measurement_secs"] == 60

        attack = json.loads(json.dumps(summary))
        attack["branches"]["branch_a"]["throughput"]["mean_messages_per_second"] = 900.0
        attack["branches"]["branch_a"]["latency_ns"]["p95"] = 300000
        comparison = compare_branch_a([summary], [attack])
        assert comparison["branch_a_impact"]["throughput_drop_percent"] == pytest.approx(10.0)
        assert comparison["branch_a_impact"]["p95_latency_increase_percent"] == pytest.approx(20.0)
        assert comparison["units"]["latency"] == "nanoseconds"

        epoch_attack = json.loads(json.dumps(attack))
        epoch_attack["branches"]["branch_a"]["latency_ns"]["p95"] = 325000
        comparisons = compare_branch_conditions(
            {
                "control": [summary],
                "panic-attack": [attack],
                "epoch-loop-attack": [epoch_attack],
            }
        )
        assert set(comparisons) == {"panic-attack", "epoch-loop-attack"}
        assert comparisons["panic-attack"]["branch_a_impact"]["throughput_drop_percent"] == pytest.approx(10.0)
        assert comparisons["epoch-loop-attack"]["branch_a_impact"]["p95_latency_increase_percent"] == pytest.approx(30.0)
        assert all(result["units"]["latency"] == "nanoseconds" for result in comparisons.values())


def test_branch_isolation_distinguishes_target_from_actual_offered_population() -> None:
    with tempfile.TemporaryDirectory() as tmp:
        root = Path(tmp)
        for branch in ("branch-a", "branch-b"):
            branch_dir = root / branch
            branch_dir.mkdir()
            branch_dir.joinpath("throughput.csv").write_text(
                "elapsed_secs,msg_count,bytes\n60.0,28000,3584000\n"
            )
            branch_dir.joinpath("sequence.csv").write_text(
                "total_expected,total_received,gap_ranges,gap_msgs,duplicates_count\n"
                "28000,28000,0,0,0\n"
            )
            branch_dir.joinpath("percentiles.json").write_text(
                json.dumps(
                    {
                        "total_count": 28_000,
                        "p50_ns": 100_000,
                        "p95_ns": 200_000,
                        "p99_ns": 300_000,
                        "p999_ns": 400_000,
                    }
                )
            )
            branch_dir.joinpath("measurement-window.json").write_text(
                json.dumps({"started_ns": 1_000_000_000, "finished_ns": 61_000_000_000})
            )

        summary = derive_branch_isolation(
            root,
            warmup_secs=30,
            measurement_secs=60,
            branch_sources={"branch-a": "source_a", "branch-b": "source_b"},
            target_messages=60_000,
        )
        branch_a = summary["branches"]["branch_a"]
        assert branch_a["target_messages"] == 60_000
        assert branch_a["offered_messages"] == 28_000
        assert branch_a["received_messages"] == 28_000
        assert branch_a["lost_messages"] == 0
        assert branch_a["target_shortfall_messages"] == 32_000


def test_branch_isolation_batch_summary_contains_both_attack_rows() -> None:
    with tempfile.TemporaryDirectory() as tmp:
        root = Path(tmp)
        batch = root / "eval/results/e-iso-7/rpi5-diagnostic"
        for condition, throughput, p95 in (
            ("control", 1000.0, 100000),
            ("panic-attack", 990.0, 110000),
            ("epoch-loop-attack", 950.0, 130000),
        ):
            run = batch / condition / "run-01-attempt-01"
            run.mkdir(parents=True)
            run.joinpath("canonical-status.json").write_text(json.dumps({"status": "passed"}))
            run.joinpath("branch-isolation.json").write_text(
                json.dumps(
                    {
                        "condition": condition,
                        "branches": {
                            "branch_a": {
                                "throughput": {"mean_messages_per_second": throughput},
                                "latency_ns": {"p95": p95},
                            }
                        },
                    }
                )
            )
        (root / "eval/results/canonical-batches/rpi5-diagnostic").mkdir(parents=True)
        path = summarize_branch_isolation(root, "diagnostic")
        summary = json.loads(path.read_text())

    assert set(summary["comparisons"]) == {"panic-attack", "epoch-loop-attack"}
    assert summary["comparisons"]["panic-attack"]["branch_a_impact"]["throughput_drop_percent"] == pytest.approx(1.0)
    assert summary["comparisons"]["epoch-loop-attack"]["branch_a_impact"]["p95_latency_increase_percent"] == pytest.approx(30.0)
    assert summary["sample_unit"] == "run"


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


def test_ekuiper_process_snapshot_rejects_overlap_and_outside_affinity() -> None:
    snapshot = json.loads(
        (ROOT / "eval/scripts/tests/fixtures/ekuiper-process-snapshot.json").read_text()
    )
    validate_ekuiper_process_snapshot(snapshot, "1-3")

    overlap = {**snapshot, "other_suts": [{"pid": 200, "name": "wafer"}]}
    with pytest.raises(ValueError, match="concurrent SUT"):
        validate_ekuiper_process_snapshot(overlap, "1-3")

    outside = {
        **snapshot,
        "processes": [
            {"pid": 100, "ppid": 1, "name": "kuiperd", "cpus_allowed_list": "0-3"}
        ],
    }
    with pytest.raises(ValueError, match="outside 1-3"):
        validate_ekuiper_process_snapshot(outside, "1-3")


def test_ekuiper_rule_and_service_dry_runs_reconstruct_matched_config() -> None:
    seed = subprocess.run(
        [str(ROOT / "eval/ekuiper/seed-pipeline-a.sh"), "--dry-run"],
        check=True,
        capture_output=True,
        text=True,
    )
    rendered = json.loads(seed.stdout)
    stream_sql = rendered["stream_payload"]["sql"]
    rule = rendered["rule_payload"]
    action = rule["actions"][0]["mqtt"]
    assert "device_id STRING" in stream_sql
    assert "humidity FLOAT" in stream_sql
    assert "DATASOURCE=\"wafer/telemetry\"" in stream_sql
    assert rule["sql"] == (
        "SELECT device_id, temperature, humidity, ts, seq FROM wafer_telemetry "
        "WHERE temperature >= 50 AND temperature <= 99999"
    )
    assert action == {
        "server": "tcp://127.0.0.1:1883",
        "topic": "wafer/telemetry/hot",
        "protocolVersion": "3.1.1",
        "qos": 1,
        "retained": False,
        "sendSingle": True,
    }
    assert rule["options"] == {"concurrency": 1}
    comparator = tomllib.loads(
        (ROOT / "eval/configs/canonical/e-perf-1-ekuiper.toml").read_text()
    )["comparator"]
    assert comparator["stream"] == stream_sql
    assert comparator["rule"] == rule["sql"]
    assert comparator["operator_concurrency"] == rule["options"]["concurrency"] == 1
    assert comparator["source_qos"] == comparator["sink_qos"] == 1
    assert comparator["source_protocol_version"] == "3.1.1"
    assert comparator["sink_protocol_version"] == "3.1.1"
    assert comparator["sink_retained"] is False
    assert comparator["send_single"] is True

    install = subprocess.run(
        [str(ROOT / "eval/ekuiper/install-native.sh"), "--dry-run"],
        check=True,
        capture_output=True,
        text=True,
    ).stdout
    assert "CPUAffinity=1 2 3" in install
    assert "MQTT_SOURCE__DEFAULT__SERVER=tcp://127.0.0.1:1883" in install
    assert "mqtt_source_config: /etc/kuiper/mqtt_source.yaml" in install
    source = (ROOT / "eval/ekuiper/mqtt-source-default.yaml").read_text()
    assert 'server: "tcp://127.0.0.1:1883"' in source
    assert "qos: 1" in source
    assert 'protocolVersion: "3.1.1"' in source

    for relative in ("eval/configs/pipeline-a-wafer.toml", "eval/configs/pipeline-a-native.toml"):
        config = tomllib.loads((ROOT / relative).read_text())
        assert config["nodes"]["mqtt-in"]["topic"] == "wafer/telemetry"
        assert config["nodes"]["mqtt-in"]["qos"] == 1
        assert config["nodes"]["mqtt-out"]["topic"] == "wafer/telemetry/hot"
        assert config["nodes"]["mqtt-out"]["qos"] == 1
        assert config["nodes"]["filter"]["config"] == {
            "field": "temperature",
            "min": 50.0,
            "max": 99999.0,
        }


def test_ekuiper_concurrency_diagnostic_changes_only_rule_concurrency() -> None:
    rendered = []
    for concurrency in (1, 3):
        result = subprocess.run(
            [
                str(ROOT / "eval/ekuiper/seed-pipeline-a.sh"),
                "--concurrency",
                str(concurrency),
                "--dry-run",
            ],
            check=True,
            capture_output=True,
            text=True,
        )
        rendered.append(json.loads(result.stdout))

    concurrency_one, concurrency_three = rendered
    assert concurrency_one["rule_payload"]["options"] == {"concurrency": 1}
    assert concurrency_three["rule_payload"]["options"] == {"concurrency": 3}
    concurrency_one["rule_payload"]["options"] = {"concurrency": 3}
    assert concurrency_one == concurrency_three


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


def test_validation_gate_accepts_hdr_bucket_containing_55_ms() -> None:
    with tempfile.TemporaryDirectory() as tmp:
        root = Path(tmp)
        run = root / "run-01-attempt-01"
        run.mkdir()
        (run / "percentiles.json").write_text(
            json.dumps({"total_count": 600, "p99_ns": 55_017_471})
        )
        (run / "canonical-status.json").write_text(json.dumps({"status": "passed"}))
        result = evaluate_validation_gate(root, expected_runs=1)
        assert result.passed is True
        assert result.failed_runs == []

        (run / "percentiles.json").write_text(
            json.dumps({"total_count": 600, "p99_ns": 55_017_472})
        )
        result = evaluate_validation_gate(root, expected_runs=1)
    assert result.passed is False
    assert result.failed_runs == [1]


if __name__ == "__main__":
    test_schedule_covers_performance_matrix()
    test_comparator_schedule_is_native_and_cross_arch_is_explicitly_partial()
    test_isolation_and_swap_schedule_preserves_experiment_semantics()
    test_isolation_derivations_use_raw_runtime_metrics()
    test_branch_isolation_uses_branch_artifacts_not_aggregate_fan_in()
    test_branch_isolation_batch_summary_contains_both_attack_rows()
    test_canonical_configs_match_frozen_windows()
    test_external_subscriber_percentiles_do_not_parse_binary_hdr()
    test_progress_log_records_counts_and_temperature_field()
    test_resume_skips_passed_attempt_and_preserves_failed_attempt()
    test_validation_gate_rejects_one_bad_repetition()
    test_validation_gate_accepts_all_repetitions()
    print("canonical runner tests: PASS")
