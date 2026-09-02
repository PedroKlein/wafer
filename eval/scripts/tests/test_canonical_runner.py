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

from canonical_runner import (  # noqa: E402
    analyze_backpressure,
    analyze_rate_sweep_traces,
    build_focused_schedule,
    build_schedule,
    classify_sustainable_throughput,
    compare_branch_a,
    compare_branch_conditions,
    copy_shared_result,
    derive_branch_isolation,
    derive_hotswap_evidence,
    derive_containment,
    evaluate_validation_gate,
    loadgen_command,
    postprocess_run,
    ProcessResourceSampler,
    RunItem,
    select_attempt,
    summarize_branch_isolation,
    summarize_process_resources,
    summarize_rate_sweep,
    summarize_recovery,
    validate_backpressure_result,
    validate_ekuiper_process_snapshot,
    validate_focused_freeze,
    validate_rate_sweep_result,
    validate_startup_artifact,
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


def test_rate_sweep_schedule_is_complete_and_position_balanced() -> None:
    schedule = build_schedule({"e-perf-10"}, seed=1729)
    systems = ("mqtt-loopback", "native", "wafer", "ekuiper")
    rates = (500, 1000, 2000, 4000, 8000, 16000)

    assert len(schedule) == 4 * len(systems) * len(rates)
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
        for run_index in range(1, 5):
            block = [
                item
                for item in schedule
                if item.run_index == run_index and item.offered_rate_msg_s == rate
            ]
            assert len(block) == len(systems)
            for position, item in enumerate(block):
                positions[position].append(item.system)
        assert all(set(observed) == set(systems) for observed in positions.values())


def test_rate_sweep_loadgen_commands_bind_rate_topics_and_raw_traces() -> None:
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
        trace_file=output / "published.csv",
    )
    subscriber = loadgen_command(
        ROOT,
        item,
        "subscribe",
        output=output,
        topic="wafer/telemetry/hot",
        trace_file=output / "received.csv",
    )

    assert publisher[publisher.index("--rate") + 1] == "4000"
    assert publisher[publisher.index("--topic") + 1] == "wafer/telemetry"
    assert publisher[publisher.index("--trace-file") + 1] == str(output / "published.csv")
    assert subscriber[subscriber.index("--total-messages") + 1] == "240000"
    assert subscriber[subscriber.index("--topic") + 1] == "wafer/telemetry/hot"
    assert subscriber[subscriber.index("--trace-file") + 1] == str(output / "received.csv")


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
    assert len(by_experiment["e-swap-4"]) == 1
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

        summary = derive_branch_isolation(root, warmup_secs=30, measurement_secs=60)
        branch_a = summary["branches"]["branch_a"]
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
