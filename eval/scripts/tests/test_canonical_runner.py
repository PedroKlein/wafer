#!/usr/bin/env python3

import contextlib
import hashlib
import io
import json
import os
import shutil
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
    estimate_candidate_capacity_envelope,
    estimate_capacity_envelope,
    apply_capacity_knee_cooldown,
    build_capacity_knee_schedule,
    build_capacity_scout_invocation,
    build_capacity_scout_rate_block,
    build_swap3_invocation,
    build_swap4_timeline,
    capacity_scout_failed_attempt_stop_reason,
    capacity_scout_next_rate,
    capacity_scout_safety_action,
    persist_capacity_scout_decision,
    replay_capacity_scout_decisions,
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
    source_state,
    summarize_branch_isolation,
    summarize_capacity_knee,
    summarize_process_resources,
    summarize_swap4_runs,
    summarize_rate_sweep,
    summarize_recovery,
    validate_backpressure_result,
    validate_capacity_scout_decision_replay,
    validate_candidate_capacity_run_result,
    validate_capacity_run_result,
    validate_capacity_scout_result,
    verify_capacity_result_files,
    validate_ekuiper_process_snapshot,
    verify_capacity_scout_result_files,
    write_capacity_scout_progress,
    write_capacity_result,
    write_capacity_scout_result,
    validate_fine_event_buckets,
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


def test_capacity_knee_dispatches_bounded_rate_sweep_runner(
    monkeypatch: pytest.MonkeyPatch, tmp_path: Path
) -> None:
    item = build_capacity_knee_schedule(seed=1729)[0]
    observed = []
    monkeypatch.setattr(
        runner,
        "run_rate_sweep_item",
        lambda root, candidate, selection: observed.append((root, candidate, selection)) or True,
    )

    assert runner.run_item(tmp_path, "test", item)
    assert len(observed) == 1


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


def test_candidate_swap_dispatches_existing_hot_swap_runner(
    monkeypatch: pytest.MonkeyPatch, tmp_path: Path
) -> None:
    observed = []
    monkeypatch.setattr(
        runner,
        "run_hot_swap_item",
        lambda root, candidate, selection: observed.append(candidate.experiment) or True,
    )
    for experiment in (runner.SWAP_SESSIONS_EXPERIMENT, runner.ROLLBACK_SESSIONS_EXPERIMENT):
        item = build_schedule({experiment}, seed=1729)[0]
        assert runner.run_item(tmp_path, "test", item)

    assert observed == [runner.SWAP_SESSIONS_EXPERIMENT, runner.ROLLBACK_SESSIONS_EXPERIMENT]


def test_candidate_swap_schedules_have_five_independent_sessions_and_fifty_events() -> None:
    swaps = build_schedule({runner.SWAP_SESSIONS_EXPERIMENT}, seed=1729)
    rollbacks = build_schedule({runner.ROLLBACK_SESSIONS_EXPERIMENT}, seed=1729)

    assert len(swaps) == len(rollbacks) == 5
    assert {item.run_index for item in swaps} == set(range(1, 6))
    assert {item.run_index for item in rollbacks} == set(range(1, 6))
    assert all(item.events_per_run == 50 for item in swaps + rollbacks)
    assert all(item.shared_from is None for item in swaps + rollbacks)
    assert all(item.measurement_secs == 120 for item in swaps)
    assert all(item.measurement_secs == 300 for item in rollbacks)


def test_candidate_swap_definitions_reject_event_class_or_alias_drift() -> None:
    matrix = json.loads((ROOT / "eval/canonical-matrix.json").read_text())
    definitions = matrix["enhanced_candidate"]["experiments"]
    for experiment in (runner.SWAP_SESSIONS_EXPERIMENT, runner.ROLLBACK_SESSIONS_EXPERIMENT):
        runner.validate_candidate_swap_definition(experiment, definitions[experiment])
        changed = json.loads(json.dumps(definitions[experiment]))
        changed["event_classes"] = ["cached"]
        with pytest.raises(ValueError, match="event_classes"):
            runner.validate_candidate_swap_definition(experiment, changed)
        changed = json.loads(json.dumps(definitions[experiment]))
        changed["no_pool_with"] = []
        with pytest.raises(ValueError, match="no_pool_with"):
            runner.validate_candidate_swap_definition(experiment, changed)


def test_candidate_runner_separates_process_cap_from_measurement_window(
    monkeypatch: pytest.MonkeyPatch, tmp_path: Path
) -> None:
    config = tmp_path / "candidate.toml"
    config.write_text("fixture")
    item = RunItem(
        experiment="e-perf-payload-refinement",
        condition="8kb",
        run_index=1,
        config="candidate.toml",
        warmup_secs=30,
        measurement_secs=60,
    )
    commands = []
    monkeypatch.setattr(runner, "set_ekuiper_active", lambda root, active: None)
    monkeypatch.setattr(
        runner.subprocess,
        "run",
        lambda command, **kwargs: commands.append(command),
    )
    monkeypatch.setattr(runner, "postprocess_run", lambda root, candidate, output: None)
    monkeypatch.setattr(runner, "verify_result", lambda root, output: None)

    assert runner.run_item(tmp_path, "test", item)

    command = commands[0]
    assert command[command.index("--duration") + 1] == "120"
    assert command[command.index("--measurement-secs") + 1] == "60"


def test_ekuiper_profile_schedule_has_exact_matched_n5_pairs() -> None:
    schedule = runner.build_ekuiper_profile_schedule(seed=1729)

    assert len(schedule) == 3 * 2 * 5
    assert len({item.result_key for item in schedule}) == len(schedule)
    assert {item.run_index for item in schedule} == set(range(1, 6))
    assert {item.condition for item in schedule} == {
        f"rate-{rate:05d}/{state}"
        for rate in (1_000, 4_000, 8_000)
        for state in ("profiled", "unprofiled-control")
    }
    assert all(item.system == "ekuiper" for item in schedule)
    assert all(item.warmup_secs == 30 for item in schedule)
    assert all(item.measurement_secs == 60 for item in schedule)
    assert all(item.total_messages == item.offered_rate_msg_s * 60 for item in schedule)
    assert all(item.exclusive_sut for item in schedule)
    for rate in (1_000, 4_000, 8_000):
        for run_index in range(1, 6):
            pair = [
                item
                for item in schedule
                if item.offered_rate_msg_s == rate and item.run_index == run_index
            ]
            assert {runner.ekuiper_profile_state(item) for item in pair} == {
                "profiled",
                "unprofiled-control",
            }
            assert len({(item.config, item.loadgen_profile) for item in pair}) == 1


def write_ekuiper_profile_fixture(
    output: Path,
    item: runner.RunItem,
    *,
    process_available: bool,
) -> dict:
    output.mkdir(parents=True)
    (output / "measurement-window.json").write_text(
        json.dumps({"started_ns": 10_000_000_000, "finished_ns": 70_000_000_000})
    )
    (output / "interval-metrics.json").write_text(
        json.dumps(
            {
                "schema_version": 1,
                "measurement_start_unix_epoch_ns": 10_000_000_000,
                "declared_measurement_duration_ns": 60_000_000_000,
                "aggregate_latency_count": item.total_messages,
                "row_count": 60,
                "rows": [
                    {
                        "interval_start_ns": index * 1_000_000_000,
                        "interval_end_ns": (index + 1) * 1_000_000_000,
                    }
                    for index in range(60)
                ],
            }
        )
    )
    (output / "percentiles.json").write_text(
        json.dumps(
            {
                "total_count": item.total_messages,
                "p50_ns": 100_000,
                "p95_ns": 200_000,
                "p99_ns": 300_000,
                "p999_ns": 400_000,
            }
        )
    )
    context = {
        "state": runner.ekuiper_profile_state(item),
        "process_profiler": {
            "status": "available" if process_available else "unavailable",
            "reason": None if process_available else "procfs-process-metrics-unavailable",
            "collection_enabled": process_available,
        },
        "gctrace": {
            "collection_enabled": runner.ekuiper_profile_state(item) == "profiled",
            "journal_readable": True,
        },
    }
    (output / "ekuiper-gctrace.log").write_text("")
    if process_available:
        (output / "resource-usage.csv").write_text(
            "timestamp_ns,cpu_time_ticks,rss_bytes,process_count,thread_count,"
            "rss_anon_bytes,rss_file_bytes,vm_data_bytes,vm_size_bytes,"
            "pss_anon_bytes,private_dirty_bytes\n"
            "10000000000,100,1000,1,8,800,200,1200,4000,700,750\n"
            "70000000000,220,1400,1,9,1100,300,1500,4400,1000,1050\n"
        )
    (output / "metadata.json").write_text(
        json.dumps(
            {
                "experiment": runner.EKUIPER_PROFILE_EXPERIMENT,
                "condition": item.condition,
                "run_index": item.run_index,
                "system": "ekuiper",
                "git_sha": "a" * 40,
                "git_dirty": False,
                "profile": context,
            }
        )
    )
    return context


def test_ekuiper_process_profiler_detects_available_and_missing_procfs(
    tmp_path: Path,
) -> None:
    process = tmp_path / "100"
    process.mkdir()
    for name, contents in {
        "stat": "100 (kuiperd) S 1 2 3 4 5 6 7 8 9 10 11 12 13 14\n",
        "statm": "10 5\n",
        "status": "RssAnon:\t1 kB\nRssFile:\t1 kB\nVmData:\t2 kB\nVmSize:\t4 kB\n",
        "smaps_rollup": "Pss_Anon:\t1 kB\nPrivate_Dirty:\t1 kB\n",
    }.items():
        (process / name).write_text(contents)
    (process / "task").mkdir()
    (process / "task/100").mkdir()

    assert runner.ekuiper_process_profile_availability([100], tmp_path) == {
        "status": "available",
        "reason": None,
        "collection_enabled": True,
    }
    assert runner.ekuiper_process_profile_availability([101], tmp_path) == {
        "status": "unavailable",
        "reason": "procfs-process-metrics-unavailable",
        "collection_enabled": False,
    }


def test_ekuiper_profile_artifacts_are_bounded_aligned_and_explicitly_diagnostic(
    tmp_path: Path,
) -> None:
    profiled = next(
        item
        for item in runner.build_ekuiper_profile_schedule(seed=1729)
        if item.condition == "rate-01000/profiled" and item.run_index == 1
    )
    context = write_ekuiper_profile_fixture(tmp_path / "profiled", profiled, process_available=True)

    runtime, overhead = runner.write_ekuiper_profile_artifacts(
        profiled, tmp_path / "profiled", context
    )

    assert runtime["evidence_class"] == "diagnostic"
    assert runtime["thesis_evidence"] is False
    assert runtime["n30_admitted"] is False
    assert runtime["rate_msg_s"] == 1_000
    assert runtime["profiler_state"] == "profiled"
    assert runtime["process_metrics"]["status"] == "available"
    assert runtime["process_metrics"]["row_count"] == 2
    assert runtime["process_metrics"]["maximum_rows"] == 62
    assert runtime["gc_runtime_metrics"] == {
        "status": "unavailable",
        "reason": "gctrace-lines-missing-from-journal",
    }
    assert runtime["interval_alignment"]["row_count"] == 60
    assert overhead["paired_condition"] == "rate-01000/unprofiled-control"
    assert overhead["overhead_role"] == "sampler-enabled"
    assert overhead["claim_boundary"] == "diagnostic-association-only-not-gc-causality"


def test_ekuiper_profile_artifacts_accept_terminal_partial_interval(
    tmp_path: Path,
) -> None:
    profiled = next(
        item
        for item in runner.build_ekuiper_profile_schedule(seed=1729)
        if item.condition == "rate-04000/unprofiled-control" and item.run_index == 1
    )
    output = tmp_path / "partial-interval"
    context = write_ekuiper_profile_fixture(output, profiled, process_available=False)
    window = json.loads((output / "measurement-window.json").read_text())
    window["finished_ns"] += 200_000_000
    (output / "measurement-window.json").write_text(json.dumps(window))
    intervals = json.loads((output / "interval-metrics.json").read_text())
    intervals["rows"].append(
        {
            "interval_start_ns": 60_000_000_000,
            "interval_end_ns": 60_000_080_274,
        }
    )
    intervals["row_count"] = 61
    (output / "interval-metrics.json").write_text(json.dumps(intervals))

    runtime, _ = runner.write_ekuiper_profile_artifacts(profiled, output, context)

    assert runtime["interval_alignment"]["row_count"] == 61


def test_ekuiper_profile_artifacts_gracefully_record_unavailable_process_metrics(
    tmp_path: Path,
) -> None:
    profiled = next(
        item
        for item in runner.build_ekuiper_profile_schedule(seed=1729)
        if item.condition == "rate-04000/profiled" and item.run_index == 2
    )
    output = tmp_path / "unavailable"
    context = write_ekuiper_profile_fixture(output, profiled, process_available=False)

    runtime, overhead = runner.write_ekuiper_profile_artifacts(profiled, output, context)

    assert runtime["process_metrics"] == {
        "status": "unavailable",
        "reason": "procfs-process-metrics-unavailable",
    }
    assert overhead["profile_collection_enabled"] is False
    assert overhead["overhead_role"] == "sampler-skipped-unavailable"


def journal_entry(timestamp_s: float, message: str) -> str:
    return json.dumps(
        {"__REALTIME_TIMESTAMP": str(round(timestamp_s * 1_000_000)), "MESSAGE": message}
    )


def gctrace_line(cycle: int, clock_ms: str, heap_mb: str, goal_mb: int) -> str:
    return (
        f"gc {cycle} @{cycle}.012s 1%: {clock_ms} ms clock, "
        f"0.06+0.45/1.8/0.62+0.012 ms cpu, {heap_mb} MB, {goal_mb} MB goal, "
        "0 MB stacks, 0 MB globals, 4 P"
    )


def fake_journalctl(
    monkeypatch: pytest.MonkeyPatch, entries: list[str], returncode: int = 0
) -> list[list[str]]:
    calls: list[list[str]] = []

    def fake_run(command: list[str], **kwargs: object) -> subprocess.CompletedProcess:
        calls.append(command)
        if kwargs.get("check") and returncode:
            raise subprocess.CalledProcessError(returncode, command)
        return subprocess.CompletedProcess(command, returncode, "\n".join(entries) + "\n", "")

    monkeypatch.setattr(runner.subprocess, "run", fake_run)
    return calls


def profile_item(condition: str) -> runner.RunItem:
    return next(
        item
        for item in runner.build_ekuiper_profile_schedule(seed=1729)
        if item.condition == condition and item.run_index == 1
    )


def test_ekuiper_gctrace_summary_covers_only_the_measurement_window(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    item = profile_item("rate-04000/profiled")
    output = tmp_path / "profiled"
    context = write_ekuiper_profile_fixture(output, item, process_available=False)
    calls = fake_journalctl(
        monkeypatch,
        [
            journal_entry(4.0, gctrace_line(1, "0.010+1.0+0.010", "4->4->1", 4)),
            journal_entry(6.0, "Started kuiper.service"),
            journal_entry(8.0, gctrace_line(2, "0.020+1.2+0.030", "4->4->2", 5)),
            journal_entry(20.0, gctrace_line(3, "0.015+2.1+0.003", "6->7->3", 8)),
            journal_entry(40.0, gctrace_line(5, "0.12+3.0+0.45", "9->10->4", 9)),
            journal_entry(75.0, gctrace_line(6, "12+4.0+0.050", "11->12->5", 10) + " (forced)"),
        ],
    )

    context["gctrace"] = runner.capture_ekuiper_gctrace(output, 5_000_000_000, True)
    runtime, _ = runner.write_ekuiper_profile_artifacts(item, output, context)

    assert calls == [
        [
            "sudo",
            "journalctl",
            "--unit=kuiper.service",
            "--since=@5",
            "--output=json",
            "--no-pager",
        ]
    ]
    log = (output / "ekuiper-gctrace.log").read_text().splitlines()
    assert [line.split(" ", 2)[:2] for line in log] == [
        ["8000000000", "gc"],
        ["20000000000", "gc"],
        ["40000000000", "gc"],
        ["75000000000", "gc"],
    ]
    assert runtime["gc_runtime_metrics"] == {
        "status": "available",
        "source": "go-gctrace-journal",
        "path": "ekuiper-gctrace.log",
        "sha256": hashlib.sha256((output / "ekuiper-gctrace.log").read_bytes()).hexdigest(),
        "trace_line_count": 4,
        "missing_cycle_count": 1,
        "cycle_count": 2,
        "stw_pause_total_ns": 15_000 + 3_000 + 120_000 + 450_000,
        "stw_pause_max_ns": 450_000,
        "max_heap_at_start_mib": 9,
        "max_live_heap_mib": 4,
        "max_heap_goal_mib": 9,
    }


@pytest.mark.parametrize(
    ("entries", "returncode", "reason"),
    [
        ([journal_entry(20.0, "Started kuiper.service")], 0, "gctrace-lines-missing-from-journal"),
        ([], 1, "kuiper-journal-unreadable"),
        ([journal_entry(20.0, "gc 3 @13.901s 1%: unexpected")], 0, "gctrace-format-unrecognized"),
    ],
)
def test_ekuiper_gctrace_problems_become_unavailable_markers(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
    entries: list[str],
    returncode: int,
    reason: str,
) -> None:
    item = profile_item("rate-01000/profiled")
    output = tmp_path / "profiled"
    context = write_ekuiper_profile_fixture(output, item, process_available=False)
    fake_journalctl(monkeypatch, entries, returncode)

    context["gctrace"] = runner.capture_ekuiper_gctrace(output, 5_000_000_000, True)
    runtime, _ = runner.write_ekuiper_profile_artifacts(item, output, context)

    assert runtime["gc_runtime_metrics"] == {"status": "unavailable", "reason": reason}


def test_ekuiper_gctrace_capture_survives_a_missing_journalctl(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    def missing(command: list[str], **kwargs: object) -> subprocess.CompletedProcess:
        raise FileNotFoundError(command[0])

    monkeypatch.setattr(runner.subprocess, "run", missing)

    assert runner.capture_ekuiper_gctrace(tmp_path, 5_000_000_000, True) == {
        "collection_enabled": True,
        "journal_readable": False,
    }
    assert (tmp_path / "ekuiper-gctrace.log").read_text() == ""


def test_unprofiled_control_rejects_gctrace_output(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    item = profile_item("rate-01000/unprofiled-control")
    output = tmp_path / "control"
    context = write_ekuiper_profile_fixture(output, item, process_available=False)
    fake_journalctl(
        monkeypatch,
        [journal_entry(20.0, gctrace_line(3, "0.015+2.1+0.003", "6->7->3", 8))],
    )

    context["gctrace"] = runner.capture_ekuiper_gctrace(output, 5_000_000_000, False)

    with pytest.raises(ValueError, match="gctrace output exists"):
        runner.write_ekuiper_profile_artifacts(item, output, context)


def candidate_ekuiper_profile_summary() -> dict:
    records = []
    for item in runner.build_ekuiper_profile_schedule(seed=1729):
        state = runner.ekuiper_profile_state(item)
        paired = "unprofiled-control" if state == "profiled" else "profiled"
        records.append(
            {
                "schema_version": 1,
                "experiment": runner.EKUIPER_PROFILE_EXPERIMENT,
                "evidence_class": "diagnostic",
                "thesis_evidence": False,
                "n30_admitted": False,
                "sample_unit": "independent host run at one rate and profiler state",
                "condition": item.condition,
                "run_index": item.run_index,
                "rate_msg_s": item.offered_rate_msg_s,
                "profiler_state": state,
                "source_git_sha": "a" * 40,
                "source_dirty": False,
                "measurement_source_leaf": f"raw/{item.result_key}",
                "shared_from": None,
                "claim_boundary": "diagnostic-association-only-not-gc-causality",
                "no_pool_with": [
                    "e-perf-1",
                    "e-perf-10",
                    "prior diagnostic rehearsals",
                ],
                "profiler_overhead": {
                    "experiment": runner.EKUIPER_PROFILE_EXPERIMENT,
                    "rate_msg_s": item.offered_rate_msg_s,
                    "profiler_state": state,
                    "run_index": item.run_index,
                    "paired_condition": (
                        f"rate-{item.offered_rate_msg_s:05d}/{paired}"
                    ),
                    "pair_key": (
                        f"rate-{item.offered_rate_msg_s:05d}/run-{item.run_index:02d}"
                    ),
                    "claim_boundary": "diagnostic-association-only-not-gc-causality",
                },
            }
        )
    return {
        "schema_version": 1,
        "experiment": runner.EKUIPER_PROFILE_EXPERIMENT,
        "batch_class": "diagnostic-ekuiper-profile",
        "evidence_class": "diagnostic",
        "thesis_evidence": False,
        "n30_admitted": False,
        "sample_unit": "independent host run at one rate and profiler state",
        "required_runs_per_cell": 5,
        "rates_msg_s": [1_000, 4_000, 8_000],
        "profiler_states": ["profiled", "unprofiled-control"],
        "complete": True,
        "claim_boundary": "diagnostic-association-only-not-gc-causality",
        "no_pool_with": ["e-perf-1", "e-perf-10", "prior diagnostic rehearsals"],
        "records": records,
    }


def test_ekuiper_profile_summary_rejects_missing_pairs_aliases_and_dirty_sources() -> None:
    summary = candidate_ekuiper_profile_summary()
    runner.validate_ekuiper_profile_summary(summary)

    summary = candidate_ekuiper_profile_summary()
    summary["records"].pop()
    with pytest.raises(ValueError, match="30 independent runs"):
        runner.validate_ekuiper_profile_summary(summary)

    summary = candidate_ekuiper_profile_summary()
    summary["records"][0]["shared_from"] = "e-perf-1"
    with pytest.raises(ValueError, match="diagnostic boundary"):
        runner.validate_ekuiper_profile_summary(summary)

    summary = candidate_ekuiper_profile_summary()
    summary["records"][0]["source_dirty"] = True
    with pytest.raises(ValueError, match="dirty source"):
        runner.validate_ekuiper_profile_summary(summary)


def test_canonical_ekuiper_items_never_enable_candidate_profiler() -> None:
    canonical = build_schedule({"e-perf-1", "e-perf-10"}, seed=1729)
    ekuiper = [item for item in canonical if item.system == "ekuiper"]
    assert all(not runner.is_ekuiper_profile_item(item) for item in ekuiper)
    for item in ekuiper:
        context, pids = runner.build_ekuiper_profile_context(
            item,
            {"process_snapshot": {"processes": [{"pid": 123}]}},
        )
        assert context is None
        assert pids == []


def test_payload_refinement_schedule_has_exact_n5_grid() -> None:
    schedule = runner.build_payload_refinement_schedule(seed=1729)

    assert len(schedule) == 10 * 5
    assert len({item.result_key for item in schedule}) == len(schedule)
    assert {item.run_index for item in schedule} == set(range(1, 6))
    assert {item.condition for item in schedule} == {
        "120b", "1kb", "8kb", "10kb", "16kb", "32kb", "64kb", "100kb", "128kb", "256kb",
    }
    assert all(item.warmup_secs == 30 for item in schedule)
    assert all(item.measurement_secs == 60 for item in schedule)
    assert all(item.loadgen_profile is None for item in schedule)
    assert all(item.total_messages is None for item in schedule)


def test_depth_extension_schedule_has_exact_n5_grid() -> None:
    schedule = runner.build_depth_extension_schedule(seed=1729)

    assert len(schedule) == 6 * 5
    assert len({item.result_key for item in schedule}) == len(schedule)
    assert {item.run_index for item in schedule} == set(range(1, 6))
    assert {item.condition for item in schedule} == {
        "depth-1", "depth-3", "depth-5", "depth-10", "depth-20", "depth-50",
    }
    assert all(item.warmup_secs == 30 for item in schedule)
    assert all(item.measurement_secs == 60 for item in schedule)
    assert all(item.shared_from is None for item in schedule)


def test_candidate_manifests_reconcile_exact_payload_and_topology() -> None:
    payload_item = next(
        item
        for item in runner.build_payload_refinement_schedule(seed=1729)
        if item.condition == "8kb"
    )
    payload = runner.build_payload_manifest(ROOT, payload_item)
    assert payload["payload_bytes"] == 8192
    assert payload["payload_sha256"] == hashlib.sha256(b"B" * 8192).hexdigest()
    assert payload["source_pattern"] == "repeated-byte-0x42"
    assert payload["sink_kind"] == "bench-sink"
    assert payload["transform_plugin_path"].endswith("wafer_pass_through.wasm")
    assert payload["edges"] == [
        {"from": "source", "to": "transform"},
        {"from": "transform", "to": "sink"},
    ]
    runner.validate_payload_manifest(payload, ROOT / payload_item.config)

    depth_item = next(
        item
        for item in runner.build_depth_extension_schedule(seed=1729)
        if item.condition == "depth-50"
    )
    topology = runner.build_topology_manifest(ROOT, depth_item)
    assert topology["depth"] == 50
    assert topology["node_count"] == 52
    assert topology["edge_count"] == 51
    assert topology["transform_count"] == 50
    assert topology["source_kind"] == "bench-source"
    assert topology["sink_kind"] == "bench-sink"
    assert topology["transform_plugin_paths"] == [runner.PASS_THROUGH_PLUGIN]
    assert topology["identical_transform_behavior"] is True
    assert topology["effective_metering_mode"] == "fuel-and-epoch"
    runner.validate_topology_manifest(topology, ROOT / depth_item.config)


def test_candidate_postprocess_stamps_metadata_and_writes_manifest(
    monkeypatch: pytest.MonkeyPatch, tmp_path: Path
) -> None:
    item = next(
        item
        for item in runner.build_payload_refinement_schedule(seed=1729)
        if item.condition == "16kb" and item.run_index == 3
    )
    (tmp_path / "metadata.json").write_text('{"experiment":"e-perf-payload-refinement"}')
    (tmp_path / "latency.hdr").write_text("fixture")

    def fake_run(command: list[str], **_: object) -> None:
        if "hdr-summary" in command:
            output = Path(command[command.index("--output") + 1])
            output.write_text(
                json.dumps(
                    {
                        "total_count": 60_000,
                        "p50_ns": 1,
                        "p95_ns": 2,
                        "p99_ns": 3,
                        "p999_ns": 4,
                    }
                )
            )

    monkeypatch.setattr(runner.subprocess, "run", fake_run)
    monkeypatch.setattr(runner, "compose_interval_metrics", lambda output, required: None)

    postprocess_run(ROOT, item, tmp_path)

    metadata = json.loads((tmp_path / "metadata.json").read_text())
    manifest = json.loads((tmp_path / "payload-manifest.json").read_text())
    assert metadata["batch_class"] == "candidate-payload-refinement"
    assert metadata["evidence_class"] == "candidate-supplementary"
    assert metadata["thesis_evidence"] is False
    assert metadata["n30_admitted"] is False
    assert manifest["condition"] == "16kb"
    assert manifest["run_index"] == 3
    assert manifest["payload_bytes"] == 16_384


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
    assert len(by_experiment["e-backpressure"]) == 90
    assert {
        (item.condition, item.run_index)
        for item in by_experiment["e-backpressure"]
    } == {
        (policy, run_index)
        for policy in ("slow", "drop", "dead-letter")
        for run_index in range(1, 31)
    }
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
                    "replacement_adopted_ns": 2_000_000,
                    "first_post_replacement_local_outcome_ns": 3_000_000,
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
                    "replacement_adopted_ns": 300_000,
                    "first_post_replacement_local_outcome_ns": 400_000,
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


def test_candidate_swap_evidence_labels_first_use_and_cached_events() -> None:
    requests = [
        {
            "event_index": index,
            "plugin": (
                "wafer_pass_through_v2.wasm"
                if index % 2 == 0
                else "wafer_pass_through_v1.wasm"
            ),
            "request_duration_ns": 100 + index,
            "request_duration_clock": "monotonic",
            "http_status": 200,
            "body": {
                "compile_cache": "compiled" if index == 0 else "memory_hit",
                "timeline": {
                    "compile_ns": 10 + index,
                    "instantiate_ns": 20 + index,
                    "signal_ns": 30 + index,
                    "replacement_adopted_ns": 40 + index,
                    "first_post_replacement_local_outcome_ns": 50 + index,
                }
            },
        }
        for index in range(50)
    ]
    sink = {"transitions": [{"pause_ns": 1_000 + index} for index in range(50)]}
    item = RunItem(
        experiment=runner.SWAP_SESSIONS_EXPERIMENT,
        condition="steady",
        run_index=3,
        config="eval/configs/e-swap/pipeline-hotswap.toml",
        warmup_secs=30,
        measurement_secs=120,
        events_per_run=50,
    )

    evidence = runner.stamp_candidate_swap_evidence(
        derive_hotswap_evidence(
            requests,
            sink,
            experiment=item.experiment,
            condition=item.condition,
            source_leaf="raw/e-swap-independent-sessions/rpi5-test/steady/run-03-attempt-01",
        ),
        item,
        {"expected": 120_000, "received": 120_000, "gaps": 0, "duplicates": 0},
    )

    assert evidence["run_index"] == 3
    assert evidence["events"][0]["event_class"] == "first-use-aot"
    assert {event["event_class"] for event in evidence["events"][1:]} == {"cached"}
    assert evidence["shared_from"] is None
    assert evidence["thesis_evidence"] is False


def test_candidate_rollback_evidence_requires_fifty_lossless_events() -> None:
    requests = [
        {
            "event_index": index,
            "plugin": "wafer_pass_through_v2_panics.wasm",
            "request_duration_ns": 100 + index,
            "request_duration_clock": "monotonic",
            "http_status": 200,
            "body": {
                "status": "rolled_back",
                "compile_cache": "compiled" if index == 0 else "memory_hit",
                "timeline": {
                    "compile_ns": 10 + index,
                    "instantiate_ns": 20 + index,
                    "signal_ns": 30 + index,
                    "rollback_ns": 40 + index,
                },
            },
        }
        for index in range(50)
    ]
    item = RunItem(
        experiment=runner.ROLLBACK_SESSIONS_EXPERIMENT,
        condition="process-trap-rollback",
        run_index=4,
        config="eval/configs/e-swap/pipeline-hotswap-rollback.toml",
        warmup_secs=30,
        measurement_secs=300,
        events_per_run=50,
    )
    sequence = {"expected": 300_000, "received": 300_000, "gaps": 0, "duplicates": 0}

    evidence = runner.build_candidate_rollback_evidence(
        requests,
        item,
        "raw/e-swap-rollback-sessions/rpi5-test/process-trap-rollback/run-04-attempt-01",
        sequence,
    )

    assert evidence["events"][0]["event_class"] == "first-use-aot"
    assert evidence["events"][49]["event_class"] == "cached"
    assert evidence["events"][0]["rollback_ns"] == 40
    with pytest.raises(ValueError, match="exactly 50"):
        runner.build_candidate_rollback_evidence(requests[:-1], item, "raw/test", sequence)


@pytest.mark.parametrize(
    ("event_index", "compile_cache"),
    [(0, "disk_hit"), (0, "memory_hit"), (7, "compiled"), (7, None)],
)
def test_candidate_swap_evidence_labels_events_from_the_reported_cache(
    event_index: int, compile_cache: str | None
) -> None:
    requests = [
        {
            "event_index": index,
            "plugin": "wafer_pass_through_v2_panics.wasm",
            "request_duration_ns": 100,
            "http_status": 200,
            "body": {
                "status": "rolled_back",
                "compile_cache": "compiled" if index == 0 else "memory_hit",
                "timeline": {"compile_ns": 1, "instantiate_ns": 2, "signal_ns": 3, "rollback_ns": 4},
            },
        }
        for index in range(50)
    ]
    item = RunItem(
        experiment=runner.ROLLBACK_SESSIONS_EXPERIMENT,
        condition="process-trap-rollback",
        run_index=1,
        config="eval/configs/e-swap/pipeline-hotswap-rollback.toml",
        warmup_secs=30,
        measurement_secs=300,
        events_per_run=50,
    )
    sequence = {"expected": 300_000, "received": 300_000, "gaps": 0, "duplicates": 0}
    requests[event_index]["body"]["compile_cache"] = compile_cache

    with pytest.raises(ValueError, match=f"event {event_index} .*compile_cache"):
        runner.build_candidate_rollback_evidence(requests, item, "raw/test", sequence)


def swap5_requests_fixture() -> list[dict]:
    return [
        {
            "event_index": index,
            "plugin": "wafer_pass_through_v2_panics.wasm",
            "request_started_ns": 1_000_000_000 + index * 6_000_000_000,
            "request_finished_ns": 1_010_000_000 + index * 6_000_000_000,
            "request_duration_ns": 10_000_000,
            "request_duration_clock": "monotonic",
            "http_status": 200,
            "body": {
                "status": "rolled_back",
                "timeline": {
                    "compile_ns": 1,
                    "instantiate_ns": 2,
                    "signal_ns": 3,
                    "rollback_ns": 4,
                },
            },
        }
        for index in range(50)
    ]


def test_swap5_rollback_and_post_rollback_continuity_reconcile() -> None:
    requests = swap5_requests_fixture()
    sequence = {"expected": 300_000, "received": 300_000, "gaps": 0, "duplicates": 0}
    intervals = {
        "rows": [
            {
                "interval_start_unix_epoch_ns": requests[-1]["request_finished_ns"],
                "interval_end_unix_epoch_ns": requests[-1]["request_finished_ns"] + 1_000_000_000,
                "throughput_messages": 1_000,
            }
        ]
    }
    rollback = runner.build_swap5_rollback(requests, sequence)
    continuity = runner.build_post_rollback_continuity(
        requests, intervals, sequence, interval_metrics_sha256="a" * 64
    )

    runner.validate_swap5_artifacts(requests, rollback, continuity, sequence)
    assert rollback["attempts"] == rollback["rolled_back"] == 50
    assert continuity["output_observed_after_final_rollback"] is True
    assert continuity["messages_after_final_rollback"] == 1_000

    fabricated = json.loads(json.dumps(continuity))
    fabricated["successful_v2_transition_observed"] = True
    with pytest.raises(ValueError, match="successful v2 transition"):
        runner.validate_swap5_artifacts(requests, rollback, fabricated, sequence)

    missing_output = json.loads(json.dumps(continuity))
    missing_output["messages_after_final_rollback"] = 0
    missing_output["output_observed_after_final_rollback"] = False
    with pytest.raises(ValueError, match="post-rollback output"):
        runner.validate_swap5_artifacts(requests, rollback, missing_output, sequence)


def candidate_swap_summary_fixture(experiment: str) -> dict:
    rollback = experiment == runner.ROLLBACK_SESSIONS_EXPERIMENT
    records = []
    for run_index in range(1, 6):
        events = []
        for event_index in range(50):
            event = {
                "event_index": event_index,
                "event_class": runner.candidate_swap_event_class(event_index),
                "plugin": (
                    "wafer_pass_through_v2_panics.wasm"
                    if rollback
                    else "wafer_pass_through_v2.wasm"
                    if event_index % 2 == 0
                    else "wafer_pass_through_v1.wasm"
                ),
                "compile_ns": 10,
                "instantiate_ns": 20,
                "signal_ns": 30,
                "http_total_ns": 100,
            }
            event["rollback_ns" if rollback else "replacement_adopted_ns"] = 40
            if not rollback:
                event["first_post_replacement_local_outcome_ns"] = 50
                event["sink_observed_output_gap_ns"] = 1_000
            events.append(event)
        record = {
            "schema_version": 1,
            "batch_class": (
                "candidate-rollback-session" if rollback else "candidate-independent-swap"
            ),
            "experiment": experiment,
            "condition": "process-trap-rollback" if rollback else "steady",
            "evidence_class": "candidate-supplementary",
            "thesis_evidence": False,
            "n30_admitted": False,
            "sample_unit": "independent host run",
            "nested_unit": "rollback event within run" if rollback else "swap event within run",
            "duration_unit": "ns",
            "sample_count": 50,
            "run_index": run_index,
            "event_classes": ["first-use-aot", "cached"],
            "shared_from": None,
            "sequence": {"expected": 100, "received": 100, "gaps": 0, "duplicates": 0},
            "no_pool_with": (
                ["e-swap-5", "prior diagnostic rehearsals"]
                if rollback
                else ["e-swap-1", "e-swap-2", "e-swap-6", "prior diagnostic rehearsals"]
            ),
            "events": events,
            "source_git_sha": "1" * 40,
            "source_dirty": False,
        }
        if rollback:
            record.update(attempts=50, rolled_back=50, all_rolled_back=True)
        records.append(record)
    return {
        "schema_version": 1,
        "experiment": experiment,
        "evidence_class": "candidate-supplementary",
        "thesis_evidence": False,
        "n30_admitted": False,
        "sample_unit": "independent host run",
        "nested_unit": "rollback event within run" if rollback else "swap event within run",
        "required_runs": 5,
        "events_per_run": 50,
        "event_classes": ["first-use-aot", "cached"],
        "complete": True,
        "no_pool_with": records[0]["no_pool_with"],
        "records": records,
    }


def test_candidate_swap_summary_rejects_duplicate_runs_and_alias_pooling() -> None:
    summary = candidate_swap_summary_fixture(runner.SWAP_SESSIONS_EXPERIMENT)
    runner.validate_candidate_swap_summary(summary)

    duplicate = json.loads(json.dumps(summary))
    duplicate["records"][1]["run_index"] = 1
    with pytest.raises(ValueError, match="missing or duplicate run identities"):
        runner.validate_candidate_swap_summary(duplicate)

    aliased = json.loads(json.dumps(summary))
    aliased["records"][0]["shared_from"] = "e-swap-1"
    with pytest.raises(ValueError, match="no-pooling boundary"):
        runner.validate_candidate_swap_summary(aliased)


def test_candidate_rollback_summary_requires_all_fifty_rollbacks_per_run() -> None:
    summary = candidate_swap_summary_fixture(runner.ROLLBACK_SESSIONS_EXPERIMENT)
    runner.validate_candidate_swap_summary(summary)

    incomplete = json.loads(json.dumps(summary))
    incomplete["records"][0]["rolled_back"] = 49
    with pytest.raises(ValueError, match="fifty successful rollbacks"):
        runner.validate_candidate_swap_summary(incomplete)


@pytest.mark.parametrize(
    ("experiment", "artifact_name", "summary_name"),
    (
        (
            runner.SWAP_SESSIONS_EXPERIMENT,
            "hotswap-analysis.json",
            "independent-swap-summary.json",
        ),
        (
            runner.ROLLBACK_SESSIONS_EXPERIMENT,
            "rollback.json",
            "rollback-session-summary.json",
        ),
    ),
)
def test_candidate_swap_summaries_select_five_passed_physical_runs(
    monkeypatch: pytest.MonkeyPatch,
    tmp_path: Path,
    experiment: str,
    artifact_name: str,
    summary_name: str,
) -> None:
    volume = tmp_path / "results"
    volume.mkdir()
    layout = runner.ResultsLayout.resolve(
        tmp_path, volume, mount_check=lambda _: True
    )
    layout.prepare()
    monkeypatch.setattr(runner, "results_layout", lambda _: layout)
    fixture = candidate_swap_summary_fixture(experiment)
    for record in fixture["records"]:
        leaf = layout.raw_path(
            experiment,
            "rpi5-test",
            "process-trap-rollback" if experiment == runner.ROLLBACK_SESSIONS_EXPERIMENT else "steady",
            f"run-{record['run_index']:02d}-attempt-01",
        )
        leaf.mkdir(parents=True)
        evidence = {
            key: value
            for key, value in record.items()
            if key not in {"source_git_sha", "source_dirty"}
        }
        (leaf / artifact_name).write_text(json.dumps(evidence))
        (leaf / "metadata.json").write_text(
            json.dumps(
                {
                    "run_index": record["run_index"],
                    "git_sha": record["source_git_sha"],
                    "git_dirty": False,
                }
            )
        )
        (leaf / "canonical-status.json").write_text('{"status":"passed"}')

    path = (
        runner.summarize_rollback_sessions(tmp_path, "test")
        if experiment == runner.ROLLBACK_SESSIONS_EXPERIMENT
        else runner.summarize_swap_sessions(tmp_path, "test")
    )
    summary = json.loads(path.read_text())

    assert path == layout.manifests / "candidate-batches/rpi5-test" / summary_name
    assert summary["complete"] is True
    assert [record["run_index"] for record in summary["records"]] == list(range(1, 6))


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
                "replacement_adopted_ns": 1,
                "first_post_replacement_local_outcome_ns": 1,
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


def test_shared_hotswap_result_preserves_one_source_without_copying_raw(tmp_path: Path) -> None:
    source = tmp_path / "eval/results/e-swap-1/rpi5-batch/steady/run-01-attempt-01"
    source.mkdir(parents=True)
    (source / "canonical-status.json").write_text('{"status":"passed"}')
    (source / "metadata.json").write_text(
        '{"experiment":"e-swap-1","evidence_class":"final","thesis_evidence":true}'
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

    receipt_path = copy_shared_result(tmp_path, "batch", item)
    receipt = json.loads(receipt_path.read_text())

    assert receipt_path == tmp_path / "eval/results/aliases/e-swap-2/rpi5-batch/steady/run-01.json"
    assert receipt["source_leaf"] == str(source.relative_to(tmp_path))
    assert receipt["sample_identity"] == receipt["source_leaf"]
    assert receipt["source_evidence_class"] == "final"
    assert receipt["independent_n_contribution"] == 0
    assert receipt["shared_measurement"] is True
    assert not (tmp_path / "eval/results/e-swap-2").exists()


def test_shared_result_uses_explicit_root_with_spaces_without_raw_copy(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    volume = tmp_path / "mounted results volume"
    volume.mkdir()
    layout = runner.ResultsLayout.resolve(
        tmp_path, volume, mount_check=lambda _: True
    )
    layout.prepare()
    monkeypatch.setattr(runner, "results_layout", lambda _: layout)
    source = layout.raw / "e-perf-1/rpi5-batch/native/run-01-attempt-01"
    source.mkdir(parents=True)
    (source / "canonical-status.json").write_text('{"status":"passed"}')
    (source / "metadata.json").write_text(
        '{"experiment":"e-perf-1","evidence_class":"final","thesis_evidence":true}'
    )
    item = RunItem(
        experiment="e-perf-2",
        condition="native",
        run_index=1,
        config="unused.toml",
        warmup_secs=30,
        measurement_secs=60,
        shared_from="e-perf-1",
    )

    receipt = copy_shared_result(tmp_path, "batch", item)

    assert receipt == volume / "manifests/aliases/e-perf-2/rpi5-batch/native/run-01.json"
    assert json.loads(receipt.read_text())["source_leaf"].startswith("raw/")
    assert not (volume / "raw/e-perf-2").exists()


def test_shared_result_rejects_nonfinal_source(tmp_path: Path) -> None:
    source = tmp_path / "eval/results/e-swap-1/rpi5-batch/steady/run-01-attempt-01"
    source.mkdir(parents=True)
    (source / "canonical-status.json").write_text('{"status":"passed"}')
    (source / "metadata.json").write_text(
        '{"experiment":"e-swap-1","evidence_class":"diagnostic","thesis_evidence":false}'
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

    with pytest.raises(ValueError, match="not final admitted evidence"):
        copy_shared_result(tmp_path, "batch", item)


def test_shared_hotswap_result_rejects_changed_immutable_source(tmp_path: Path) -> None:
    source = tmp_path / "eval/results/e-swap-1/rpi5-batch/steady/run-01-attempt-01"
    source.mkdir(parents=True)
    status = source / "canonical-status.json"
    status.write_text('{"status":"passed"}')
    (source / "metadata.json").write_text(
        '{"experiment":"e-swap-1","evidence_class":"final","thesis_evidence":true}'
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
    copy_shared_result(tmp_path, "batch", item)
    status.write_text('{"status":"passed","changed":true}')

    with pytest.raises(ValueError, match="alias receipt differs"):
        copy_shared_result(tmp_path, "batch", item)


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
    publisher = {"intended": 120_000, "rejected": 0, "enqueued": 120_000, "acked": 120_000, "unacked_at_exit": 0, "connects": 1}
    subscriber = {
        "total_recorded": received,
        "latency_p50_ns": 100,
        "latency_p95_ns": 200,
        "latency_p99_ns": 300,
        "sequence": {"total_received": received, "total_duplicates": 0},
    }
    return throughput, timeline, publisher, subscriber


def fine_event_fixture(
    canonical: dict,
    *,
    experiment: str,
    event_timestamp_ns: int,
    scheduled_timestamp_ns: int,
) -> dict:
    fine = []
    parents = []
    for index in range(400):
        start = -2_000_000_000 + index * 10_000_000
        unique = 10 if experiment == "e-swap-3" else 1
        duplicate = 0
        fine.append({
            "start_offset_ns": start,
            "end_offset_ns": start + 10_000_000,
            "received_unique": unique,
            "received_events": unique + duplicate,
            "duplicates": duplicate,
            "rate_msg_s": unique * 100,
        })
    for index in range(40):
        nested = fine[index * 10 : (index + 1) * 10]
        start = -2_000_000_000 + index * 100_000_000
        unique = sum(row["received_unique"] for row in nested)
        events = sum(row["received_events"] for row in nested)
        duplicates = sum(row["duplicates"] for row in nested)
        parents.append({
            "start_offset_ns": start,
            "end_offset_ns": start + 100_000_000,
            "received_unique": unique,
            "received_events": events,
            "duplicates": duplicates,
            "rate_msg_s": unique * 10,
        })
    totals = {
        field: sum(row[field] for row in fine)
        for field in ("received_unique", "received_events", "duplicates")
    }
    return {
        "schema_version": 1,
        "clock": "unix-epoch" if experiment == "e-swap-3" else "unix-epoch-source-sink-alignment",
        "clock_purpose": "cross-process-alignment",
        "alignment": "actual-t0",
        "source_measurement_start_unix_ns": canonical.get("source_measurement_start_unix_ns"),
        "scheduled_event_timestamp_ns": scheduled_timestamp_ns,
        "event_timestamp_ns": event_timestamp_ns,
        "alignment_error_ns": event_timestamp_ns - scheduled_timestamp_ns,
        "alignment_tolerance_ns": 10_000_000,
        "bucket_width_ns": 10_000_000,
        "bucket_count": 400,
        "coverage_start_offset_ns": -2_000_000_000,
        "coverage_end_offset_ns": 2_000_000_000,
        "parent_bucket_width_ns": 100_000_000,
        "parent_bucket_count": 40,
        **totals,
        "buckets": fine,
        "parent_buckets": parents,
        "canonical_series": "throughput-buckets.json",
        "loss_accounting": "canonical-sequence-and-primary-drain-only",
    }


def test_swap3_fine_event_buckets_reconcile_to_canonical_actual_t0_slice() -> None:
    throughput, timeline, _, _ = swap3_fixture()
    fine = fine_event_fixture(
        throughput,
        experiment="e-swap-3",
        event_timestamp_ns=timeline["event_timestamp_ns"],
        scheduled_timestamp_ns=timeline["scheduled_event_timestamp_ns"],
    )

    validate_fine_event_buckets(fine, throughput, experiment="e-swap-3")

    fine["buckets"][0]["received_events"] += 1
    with pytest.raises(ValueError, match="contiguous or reconciled"):
        validate_fine_event_buckets(fine, throughput, experiment="e-swap-3")


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
    assert all(
        invocation["controlled_factors"]["action_timing_receipt"]
        == "disruption-timeline.json"
        for invocation in invocations
    )
    assert all(invocation["controlled_factors"]["event_offset_ns"] == 60_000_000_000 for invocation in invocations)


def test_swap3_postprocess_removes_publisher_timing_temporary(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    item = next(
        candidate
        for candidate in build_schedule({"e-swap-3"}, seed=1729)
        if candidate.condition == "wafer-hotswap" and candidate.run_index == 1
    )
    throughput, timeline, publisher, subscriber = swap3_fixture()
    fine = fine_event_fixture(
        throughput,
        experiment="e-swap-3",
        event_timestamp_ns=timeline["event_timestamp_ns"],
        scheduled_timestamp_ns=timeline["scheduled_event_timestamp_ns"],
    )
    (tmp_path / "metadata.json").write_text(json.dumps({"experiment": "e-swap-3"}))
    (tmp_path / "publisher-summary.json").write_text(
        json.dumps(
            {
                "schema_version": 1,
                "measurement_duration_ns": 120_000_000_000,
                "deadline_misses": 0,
                **publisher,
            }
        )
    )
    (tmp_path / "subscriber-metadata.json").write_text(json.dumps(subscriber))
    (tmp_path / "throughput-buckets.json").write_text(json.dumps(throughput))
    (tmp_path / "throughput-buckets-10ms.json").write_text(json.dumps(fine))
    (tmp_path / "disruption-timeline.json").write_text(json.dumps(timeline))
    (tmp_path / "publisher-timing.json").write_text(
        json.dumps({"event_unix_epoch_ns": timeline["event_timestamp_ns"]})
    )

    monkeypatch.setattr(runner.subprocess, "run", lambda *args, **kwargs: None)
    monkeypatch.setattr(runner, "compose_interval_metrics", lambda output, required: None)

    postprocess_run(ROOT, item, tmp_path)

    assert not (tmp_path / "publisher-timing.json").exists()
    assert (tmp_path / "disruption-analysis.json").is_file()


def test_swap3_stops_subscriber_when_publisher_window_ends(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    output = tmp_path / "attempt"
    config = tmp_path / "ekuiper.toml"
    config.write_text("[comparator]\n")
    item = RunItem(
        experiment="e-swap-3",
        condition="ekuiper-restart",
        run_index=1,
        config=config.name,
        warmup_secs=30,
        measurement_secs=120,
        loadgen_profile="unused.toml",
        total_messages=120_000,
        system="ekuiper",
    )
    event_ns = 61_000_000_000
    times = iter((0, 1_000_000_000, event_ns, event_ns, event_ns + 1_000_000))
    class Process:
        def __init__(self, returncode: int) -> None:
            self.returncode = returncode

        def wait(self, timeout: int) -> int:
            return self.returncode

        def poll(self) -> int:
            return self.returncode

    subscriber = Process(1)
    publisher = Process(0)

    def fake_run(command: list[str], **kwargs: object) -> subprocess.CompletedProcess:
        if "validate-canonical.py" in " ".join(command):
            output_arg = Path(command[command.index("--output") + 1])
            output_arg.write_text(json.dumps({"git_sha": "test-sha"}))
        return subprocess.CompletedProcess(command, 0)

    def fake_popen(command: list[str], **kwargs: object) -> object:
        if command == ["subscribe"]:
            return subscriber
        (output / "publisher-timing.json").write_text(
            json.dumps(
                {
                    "measurement_started_unix_epoch_ns": 1_000_000_000,
                    "event_unix_epoch_ns": event_ns,
                    "event_offset_ns": 60_000_000_000,
                }
            )
        )
        return publisher

    observed_timeouts: list[int] = []

    def stop_after_observation(process: object, timeout: int = 30) -> int:
        assert process is subscriber
        observed_timeouts.append(timeout)
        return subscriber.returncode

    def fake_audit(root: Path, destination: Path, cpus: str) -> Path:
        path = destination / "ekuiper-audit.json"
        path.write_text("{}\n")
        return path

    ekuiper_states: list[bool] = []
    monkeypatch.setattr(runner, "start_pi_telemetry", lambda root, destination, item=None: [])
    monkeypatch.setattr(runner, "stop_pi_telemetry", lambda telemetry: None)
    monkeypatch.setattr(runner, "set_ekuiper_active", lambda root, active: ekuiper_states.append(active))
    monkeypatch.setattr(runner, "capture_ekuiper_audit", fake_audit)
    monkeypatch.setattr(runner, "loadgen_command", lambda root, candidate, action, **kwargs: [action])
    monkeypatch.setattr(runner.subprocess, "run", fake_run)
    monkeypatch.setattr(runner.subprocess, "Popen", fake_popen)
    monkeypatch.setattr(runner, "wait_for_ekuiper_rule_ready", lambda rule: None)
    monkeypatch.setattr(runner, "wait_for_subscriber", stop_after_observation)
    monkeypatch.setattr(runner.time, "time_ns", lambda: next(times))
    monkeypatch.setattr(runner.time, "sleep", lambda seconds: None)

    assert not runner.run_restart_item(
        tmp_path,
        item,
        runner.AttemptSelection(path=output, skip=False),
    )

    assert observed_timeouts == [0]
    assert ekuiper_states == [True, False]


def test_both_host_samplers_pin_themselves_to_the_support_cpus(tmp_path: Path, monkeypatch) -> None:
    commands: list[list[str]] = []
    monkeypatch.setattr(runner.subprocess, "Popen", lambda command, **kwargs: commands.append(command))
    item = RunItem("e-perf-1", "wafer", 1, "c.toml", 30, 60, runtime_cpus="1-3", support_cpus="0")
    runner.start_pi_telemetry(tmp_path, tmp_path / "leaf", item)
    assert [Path(command[1]).name for command in commands] == ["pi_telemetry.py", "proc_telemetry.py"]
    for command in commands:
        assert command[command.index("--pin-cpus") + 1] == "0"


@pytest.mark.parametrize(
    ("state", "gctrace"), [("unprofiled-control", False), ("profiled", True)]
)
def test_ekuiper_profile_stops_subscriber_when_publisher_window_ends(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch, state: str, gctrace: bool
) -> None:
    output = tmp_path / "attempt"
    config = tmp_path / "ekuiper.toml"
    config.write_text("[comparator]\n")
    item = RunItem(
        experiment="e-compare-ekuiper-profile",
        condition=f"rate-08000/{state}",
        run_index=1,
        config=config.name,
        warmup_secs=30,
        measurement_secs=60,
        loadgen_profile="unused.toml",
        offered_rate_msg_s=8_000,
        total_messages=480_000,
        system="ekuiper",
    )

    class Process:
        returncode = 1

    subscriber = Process()
    observed_timeouts: list[int] = []
    ekuiper_states: list[tuple[bool, bool]] = []
    audited_gctrace: list[bool] = []

    def fake_run(command: list[str], **kwargs: object) -> subprocess.CompletedProcess:
        if "validate-canonical.py" in " ".join(command):
            output_arg = Path(command[command.index("--output") + 1])
            output_arg.write_text(json.dumps({"git_sha": "test-sha"}))
        return subprocess.CompletedProcess(command, 0)

    def fake_audit(root: Path, destination: Path, cpus: str, gctrace: bool = False) -> Path:
        audited_gctrace.append(gctrace)
        path = destination / "ekuiper-audit.json"
        path.write_text('{"process_snapshot":{"processes":[]}}\n')
        return path

    def stop_after_observation(process: object, timeout: int = 30) -> int:
        assert process is subscriber
        observed_timeouts.append(timeout)
        return subscriber.returncode

    monkeypatch.setattr(runner, "start_pi_telemetry", lambda root, destination, item=None: [])
    monkeypatch.setattr(runner, "stop_pi_telemetry", lambda telemetry: None)
    monkeypatch.setattr(
        runner,
        "set_ekuiper_active",
        lambda root, active, gctrace=False: ekuiper_states.append((active, gctrace)),
    )
    monkeypatch.setattr(runner, "capture_ekuiper_audit", fake_audit)
    monkeypatch.setattr(runner, "build_ekuiper_profile_context", lambda item, audit: (None, []))
    monkeypatch.setattr(runner, "loadgen_command", lambda root, candidate, action, **kwargs: [action])
    monkeypatch.setattr(runner.subprocess, "run", fake_run)
    monkeypatch.setattr(runner.subprocess, "Popen", lambda *args, **kwargs: subscriber)
    monkeypatch.setattr(runner, "wait_for_subscriber", stop_after_observation)
    monkeypatch.setattr(runner.time, "sleep", lambda seconds: None)

    assert not runner.run_ekuiper_item(
        tmp_path,
        item,
        runner.AttemptSelection(path=output, skip=False),
    )

    assert observed_timeouts == [0]
    assert ekuiper_states == [(True, gctrace), (False, False)]
    assert audited_gctrace == [gctrace]


def test_profiled_ekuiper_run_reads_gctrace_after_ekuiper_stops(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    output = tmp_path / "attempt"
    config = tmp_path / "ekuiper.toml"
    config.write_text("[comparator]\n")
    item = RunItem(
        experiment="e-compare-ekuiper-profile",
        condition="rate-01000/profiled",
        run_index=1,
        config=config.name,
        warmup_secs=30,
        measurement_secs=60,
        loadgen_profile="unused.toml",
        offered_rate_msg_s=1_000,
        total_messages=60_000,
        system="ekuiper",
    )
    events: list[tuple] = []

    class Process:
        returncode = 0

    def fake_run(command: list[str], **kwargs: object) -> subprocess.CompletedProcess:
        if "validate-canonical.py" in " ".join(command):
            output_arg = Path(command[command.index("--output") + 1])
            output_arg.write_text(json.dumps({"git_sha": "test-sha"}))
        return subprocess.CompletedProcess(command, 0)

    def fake_audit(root: Path, destination: Path, cpus: str, gctrace: bool = False) -> Path:
        path = destination / "ekuiper-audit.json"
        path.write_text('{"process_snapshot":{"processes":[]}}\n')
        return path

    def fake_capture(destination: Path, since_ns: int, enabled: bool) -> dict:
        events.append(("journal", enabled))
        return {"collection_enabled": enabled, "journal_readable": True}

    monkeypatch.setattr(runner, "start_pi_telemetry", lambda root, destination, item=None: [])
    monkeypatch.setattr(runner, "stop_pi_telemetry", lambda telemetry: None)
    monkeypatch.setattr(
        runner,
        "set_ekuiper_active",
        lambda root, active, gctrace=False: events.append(("active", active, gctrace)),
    )
    monkeypatch.setattr(runner, "capture_ekuiper_audit", fake_audit)
    monkeypatch.setattr(runner, "capture_ekuiper_gctrace", fake_capture)
    monkeypatch.setattr(runner, "loadgen_command", lambda root, candidate, action, **kwargs: [action])
    monkeypatch.setattr(runner.subprocess, "run", fake_run)
    monkeypatch.setattr(runner.subprocess, "Popen", lambda *args, **kwargs: Process())
    monkeypatch.setattr(runner, "wait_for_subscriber", lambda process, timeout=30: 0)
    monkeypatch.setattr(runner.time, "sleep", lambda seconds: None)
    monkeypatch.setattr(runner, "postprocess_run", lambda root, candidate, destination: None)
    monkeypatch.setattr(runner, "verify_result", lambda root, destination: None)

    assert runner.run_ekuiper_item(
        tmp_path,
        item,
        runner.AttemptSelection(path=output, skip=False),
    )

    assert events == [("active", True, True), ("active", False, False), ("journal", True)]
    metadata = json.loads((output / "metadata.json").read_text())
    assert metadata["profile"]["gctrace"] == {
        "collection_enabled": True,
        "journal_readable": True,
    }


@pytest.fixture
def systemctl_calls(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> tuple[Path, list[list[str]]]:
    drop_in = tmp_path / "run/kuiper.service.d/wafer-gctrace.conf"
    calls: list[list[str]] = []

    def fake_run(command: list[str], **kwargs: object) -> subprocess.CompletedProcess:
        calls.append(command)
        if command[:2] == ["sudo", "install"]:
            drop_in.parent.mkdir(parents=True, exist_ok=True)
            drop_in.write_bytes(Path(command[-2]).read_bytes())
        elif command[:3] == ["sudo", "rm", "-f"]:
            drop_in.unlink(missing_ok=True)
        return subprocess.CompletedProcess(command, 0)

    monkeypatch.setattr(runner, "EKUIPER_GCTRACE_DROP_IN", drop_in)
    monkeypatch.setattr(runner.subprocess, "run", fake_run)
    return drop_in, calls


def test_profiled_ekuiper_start_traces_gc_and_stop_removes_the_drop_in(
    systemctl_calls: tuple[Path, list[list[str]]],
) -> None:
    drop_in, calls = systemctl_calls

    runner.set_ekuiper_active(ROOT, True, gctrace=True)

    assert drop_in.read_text() == "[Service]\nEnvironment=GODEBUG=gctrace=1\n"
    assert [command[:3] for command in calls[:4]] == [
        ["sudo", "systemctl", "stop"],
        ["sudo", "install", "-D"],
        ["sudo", "systemctl", "daemon-reload"],
        ["sudo", "systemctl", "start"],
    ]
    assert calls[4][0].endswith("seed-pipeline-a.sh")

    calls.clear()
    runner.set_ekuiper_active(ROOT, False)

    assert not drop_in.exists()
    assert calls == [
        ["sudo", "systemctl", "stop", "kuiper.service"],
        ["sudo", "rm", "-f", str(drop_in)],
        ["sudo", "systemctl", "daemon-reload"],
    ]


def test_ekuiper_start_without_gctrace_removes_a_leftover_drop_in(
    systemctl_calls: tuple[Path, list[list[str]]],
) -> None:
    drop_in, calls = systemctl_calls
    drop_in.parent.mkdir(parents=True)
    drop_in.write_text("[Service]\nEnvironment=GODEBUG=gctrace=1\n")

    runner.set_ekuiper_active(ROOT, True)

    assert not drop_in.exists()
    assert calls[:4] == [
        ["sudo", "systemctl", "stop", "kuiper.service"],
        ["sudo", "rm", "-f", str(drop_in)],
        ["sudo", "systemctl", "daemon-reload"],
        ["sudo", "systemctl", "start", "kuiper.service"],
    ]


def test_ekuiper_start_without_gctrace_keeps_the_plain_start_sequence(
    systemctl_calls: tuple[Path, list[list[str]]],
) -> None:
    _, calls = systemctl_calls

    runner.set_ekuiper_active(ROOT, True)
    runner.set_ekuiper_active(ROOT, False)

    assert calls[0] == ["sudo", "systemctl", "start", "kuiper.service"]
    assert calls[1][0].endswith("seed-pipeline-a.sh")
    assert calls[2:] == [["sudo", "systemctl", "stop", "kuiper.service"]]


@pytest.mark.parametrize(
    ("environment", "gctrace"),
    [
        ("HOME=/var/lib/kuiper GODEBUG=gctrace=1", False),
        ("HOME=/var/lib/kuiper", True),
        ("HOME=/var/lib/kuiper GODEBUG=gctrace=1,madvdontneed=1", True),
    ],
)
def test_ekuiper_audit_rejects_godebug_that_does_not_match_the_run(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch, environment: str, gctrace: bool
) -> None:
    monkeypatch.setattr(
        runner,
        "_service_properties",
        lambda: {"MainPID": "100", "Environment": environment},
    )

    with pytest.raises(ValueError, match="GODEBUG"):
        runner.capture_ekuiper_audit(ROOT, tmp_path, "1-3", gctrace=gctrace)


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
        "body": {"timeline": {field: 1 for field in ("compile_ns", "instantiate_ns", "signal_ns", "replacement_adopted_ns", "first_post_replacement_local_outcome_ns")}},
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



def swap4_actual_t0_fixture(timing: dict, requests: list[dict]) -> dict:
    return {
        "schema_version": 1,
        "clock": "unix-epoch",
        "alignment": "actual-t0",
        "source_measurement_start_unix_ns": timing["measurement_start_ns"],
        "scheduled_event_timestamp_ns": timing["scheduled_swap_ns"],
        "event_timestamp_ns": requests[0]["request_started_ns"],
        "alignment_error_ns": requests[0]["request_started_ns"] - timing["scheduled_swap_ns"],
        "alignment_tolerance_ns": 10_000_000,
    }



def test_swap4_fine_event_buckets_are_nested_without_replacing_tail_accounting() -> None:
    timing, _, requests, _, throughput, _ = swap4_fixture()
    fine = fine_event_fixture(
        throughput,
        experiment="e-swap-4",
        event_timestamp_ns=requests[0]["request_started_ns"],
        scheduled_timestamp_ns=timing["scheduled_swap_ns"],
    )

    validate_fine_event_buckets(fine, throughput, experiment="e-swap-4")
    assert fine["received_events"] < throughput["received_events"]
    assert throughput["primary_buckets"][-1]["received_events"] == 99
    assert throughput["drain_received_events"] == 1
    assert throughput["after_drain_events"] == 0
    assert throughput["drain_right_censored"] is False

    fine["parent_buckets"][0]["received_unique"] += 1
    with pytest.raises(ValueError, match="contiguous or reconciled|fine and parent|parent population"):
        validate_fine_event_buckets(fine, throughput, experiment="e-swap-4")


def test_swap4_schedule_has_30_runs_with_one_event_at_measured_t60() -> None:
    runs = build_schedule({"e-swap-4"}, seed=1729)
    assert len(runs) == 30
    assert {run.run_index for run in runs} == set(range(1, 31))
    assert all(hot_swap_offsets(run) == [60.0] for run in runs)


def test_swap4_timeline_requires_one_centered_swap_and_reconciled_phases() -> None:
    timing, source, requests, sink, throughput, sequence = swap4_fixture()
    actual_t0_receipt = swap4_actual_t0_fixture(timing, requests)
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
    validate_swap4_artifacts(timeline, throughput, requests, sink, actual_t0_receipt)

    invalid_receipt = json.loads(json.dumps(actual_t0_receipt))
    invalid_receipt["alignment_error_ns"] += 1
    with pytest.raises(ValueError, match="actual-t0 receipt"):
        validate_swap4_artifacts(timeline, throughput, requests, sink, invalid_receipt)
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
    assert summary["runs_with_drain_arrivals"] == 15
    assert summary["max_drain_arrival_offset_ns"] == 120_000_000_029

    runs[0]["hotswap_analysis"]["events"].append({"sink_observed_output_gap_ns": 1})
    with pytest.raises(ValueError, match="one event"):
        summarize_swap4_runs(runs)
    runs = [
        {
            "run_index": index,
            "burst_timeline": {
                "successful_swaps": 1,
                "sequence": {"gaps": 0, "duplicates": 0},
                "drain_received_events": 0,
                "drain_last_offset_ns": None,
                "drain_right_censored": False,
            },
            "hotswap_analysis": {"sample_count": 1, "events": [{"sink_observed_output_gap_ns": index * 1_000_000}]},
        }
        for index in range(1, 31)
    ]
    runs[0]["run_index"] = 30
    with pytest.raises(ValueError, match="30 independent run indices"):
        summarize_swap4_runs(runs)


def queue_sample(elapsed_ns: int, depth: int, accepted: int, processed: int) -> dict:
    return {
        "elapsed_ns": elapsed_ns,
        "queue": "slow",
        "depth": depth,
        "capacity": 64,
        "accepted": accepted,
        "dequeued": processed,
        "processed": processed,
        "dropped": 0,
        "dead_lettered": 0,
        "downstream_closed": 0,
        "dlq_full": 0,
        "dlq_closed": 0,
    }


def test_backpressure_requires_observed_queue_pressure_and_recovery() -> None:
    samples = [
        queue_sample(0, 0, 0, 0),
        queue_sample(100_000_000, 60, 100, 40),
        queue_sample(200_000_000, 64, 140, 76),
        queue_sample(300_000_000, 4, 140, 136),
        queue_sample(400_000_000, 0, 140, 140),
    ]

    result = analyze_backpressure(
        samples,
        policy="slow",
        measured_queue="slow",
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
        "elapsed_ns,queue,depth,capacity,accepted,dequeued,processed,dropped,dead_lettered,downstream_closed,dlq_full,dlq_closed\n"
        "0,slow,0,64,0,0,0,0,0,0,0,0\n"
        "100000000,slow,64,64,500,436,435,0,0,0,0,0\n"
        "1000000000,slow,0,64,1000,1000,1000,0,0,0,0,0\n"
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
        condition="slow",
        run_index=1,
        config="eval/configs/e-backpressure/pipeline-saturated.toml",
        warmup_secs=0,
        measurement_secs=10,
        total_messages=1000,
    )

    postprocess_run(ROOT, item, tmp_path)

    result = json.loads((tmp_path / "backpressure.json").read_text())
    assert result["classification"] == "saturated-and-drained"
    assert result["policy"] == "slow"
    assert result["accounting"]["reconciled"] is True
    assert result["counts"]["attempted"] == result["counts"]["delivered"] == 1_000
    assert result["memory"]["within_limit"] is True
    assert set(result["rates_msg_s"]) == {"offered", "accepted", "processed", "drained"}


def test_backpressure_postprocess_accounts_for_trailing_drops(tmp_path: Path) -> None:
    (tmp_path / "metadata.json").write_text("{}")
    (tmp_path / "queue-depth.csv").write_text(
        "elapsed_ns,queue,depth,capacity,accepted,dequeued,processed,dropped,dead_lettered,downstream_closed,dlq_full,dlq_closed\n"
        "0,slow,0,64,0,0,0,0,0,0,0,0\n"
        "100000000,slow,64,64,500,436,435,0,0,0,0,0\n"
        "1000000000,slow,0,64,700,700,700,300,0,0,0,0\n"
    )
    (tmp_path / "memory.csv").write_text(
        "elapsed_ms,rss_bytes\n0,100000000\n1000,110000000\n"
    )
    (tmp_path / "sequence.csv").write_text(
        "total_expected,total_received,gap_ranges,gap_msgs,duplicates_count\n"
        "700,700,0,0,0\n"
    )
    item = RunItem(
        experiment="e-backpressure",
        condition="drop",
        run_index=1,
        config="eval/configs/e-backpressure/pipeline-drop.toml",
        warmup_secs=0,
        measurement_secs=10,
        total_messages=1000,
    )

    postprocess_run(ROOT, item, tmp_path)

    result = json.loads((tmp_path / "backpressure.json").read_text())
    assert result["counts"]["attempted"] == 1_000
    assert result["counts"]["delivered"] == 700
    assert result["counts"]["dropped"] == 300
    assert result["sequence"] == {
        "offered": 1_000,
        "received": 700,
        "gaps": 300,
        "duplicates": 0,
    }

    (tmp_path / "sequence.csv").write_text(
        "total_expected,total_received,gap_ranges,gap_msgs,duplicates_count\n"
        "699,700,0,0,0\n"
    )
    with pytest.raises(ValueError, match="observed sequence span"):
        postprocess_run(ROOT, item, tmp_path)


def policy_backpressure_result(policy: str) -> dict:
    counts = {
        "attempted": 1_000,
        "accepted": 1_000,
        "processed": 1_000,
        "delivered": 1_000,
        "dropped": 0,
        "dead_lettered": 0,
        "downstream_closed": 0,
        "dlq_full": 0,
        "dlq_closed": 0,
        "outstanding": 0,
    }
    if policy == "drop":
        counts.update(accepted=700, processed=700, delivered=700, dropped=300)
    elif policy == "dead-letter":
        counts.update(
            accepted=700,
            processed=700,
            delivered=700,
            dead_lettered=250,
            dlq_full=40,
            dlq_closed=10,
        )
    gaps = counts["attempted"] - counts["delivered"]
    return {
        "schema_version": 2,
        "experiment": "e-backpressure",
        "condition": policy,
        "run_index": 1,
        "sample_unit": "run",
        "policy": policy,
        "queue": "slow",
        "classification": "saturated-and-drained",
        "threshold_crossed": True,
        "recovered": True,
        "peak_occupancy": 1.0,
        "occupancy_threshold": 0.8,
        "recovery_threshold": 0.1,
        "counts": counts,
        "rates_msg_s": {
            "offered": 1_000.0,
            "accepted": float(counts["accepted"]),
            "processed": float(counts["processed"]),
            "drained": 1_000.0,
        },
        "sequence": {
            "offered": 1_000,
            "received": counts["delivered"],
            "gaps": gaps,
            "duplicates": 0,
        },
        "accounting": {
            "reconciled": True,
            "dlq_failures": {
                "full": counts["dlq_full"],
                "closed": counts["dlq_closed"],
                "total": counts["dlq_full"] + counts["dlq_closed"],
            },
        },
        "producer_progress": "backpressured" if policy == "slow" else "nonblocking",
        "memory": {"within_limit": True},
    }


def test_backpressure_validates_each_policy_without_universal_losslessness() -> None:
    equations = {
        "slow": "attempted = delivered",
        "drop": "attempted = delivered + dropped",
        "dead-letter": "attempted = delivered + dead_lettered + dlq_full + dlq_closed",
    }
    for policy, equation in equations.items():
        result = policy_backpressure_result(policy)
        result["accounting"]["equation"] = equation
        validate_backpressure_result(result, policy)


def test_backpressure_dead_letter_requires_visible_dlq_failure_accounting() -> None:
    result = policy_backpressure_result("dead-letter")
    result["accounting"]["equation"] = (
        "attempted = delivered + dead_lettered + dlq_full + dlq_closed"
    )
    validate_backpressure_result(result, "dead-letter")

    hidden = json.loads(json.dumps(result))
    hidden["accounting"]["dlq_failures"] = {"full": 0, "closed": 0, "total": 0}
    with pytest.raises(ValueError, match="DLQ failure accounting"):
        validate_backpressure_result(hidden, "dead-letter")

    missing_equation = json.loads(json.dumps(result))
    del missing_equation["accounting"]["equation"]
    with pytest.raises(ValueError, match="accounting equation"):
        validate_backpressure_result(missing_equation, "dead-letter")


def test_backpressure_does_not_infer_saturation_from_offered_rate() -> None:
    samples = [
        queue_sample(0, 0, 0, 0),
        queue_sample(100_000_000, 0, 100, 100),
    ]

    result = analyze_backpressure(
        samples,
        policy="slow",
        measured_queue="slow",
        offered_messages=10_000,
        offered_duration_ns=100_000_000,
        occupancy_threshold=0.8,
        recovery_threshold=0.1,
    )

    assert result["classification"] == "not-saturated"
    assert result["threshold_crossed"] is False
    assert result["rates_msg_s"]["offered"] == 100_000.0
    assert result["rates_msg_s"]["accepted"] == 1000.0
    result["condition"] = "slow"
    result["run_index"] = 1
    with pytest.raises(ValueError, match="did not cross"):
        validate_backpressure_result(result, "slow")


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


def test_postprocessing_labels_final_matrix_leaves_as_admitted_evidence(tmp_path: Path) -> None:
    (tmp_path / "metadata.json").write_text("{}")
    (tmp_path / "startup.json").write_text(json.dumps(_startup_artifact()))
    item = RunItem(
        experiment="e-perf-9",
        condition="small-warm",
        run_index=1,
        config="eval/configs/e-perf-9/pipeline-tier-small.toml",
        warmup_secs=0,
        measurement_secs=0,
        startup_mode="warm",
    )

    postprocess_run(ROOT, item, tmp_path)

    metadata = json.loads((tmp_path / "metadata.json").read_text())
    assert metadata["evidence_class"] == "final"
    assert metadata["thesis_evidence"] is True


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
            "acked": 3990,
            "received_events": 3985,
            "received_unique": 3980,
            "downstream_lost": 10,
            "total_undelivered": 20,
            "duplicates": 5,
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
    publisher = {"intended": 4000, "rejected": 10, "enqueued": 3990, "acked": 3990, "unacked_at_exit": 0, "connects": 1}
    subscriber = {
        "total_recorded": 3985,
        "parse_errors": 0,
        "negative_latency_count": 0, "above_highest_latency_count": 0, "clock_steps": 0,
        "ignored_sequence_count": 0,
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
        (output / "publisher-summary.json").write_text(json.dumps({"intended": 4000, "rejected": 10, "enqueued": 3990, "acked": 3990, "unacked_at_exit": 0, "connects": 1}))
        (output / "subscriber-metadata.json").write_text(json.dumps({
            "total_recorded": 3985,
            "total_messages": 3985,
            "parse_errors": 0,
            "negative_latency_count": 0, "above_highest_latency_count": 0, "clock_steps": 0,
            "ignored_sequence_count": 0,
            "latency_p50_ns": 100,
            "latency_p95_ns": 200,
            "latency_p99_ns": 300,
            "histogram_lowest_ns": 1_000,
            "histogram_highest_ns": 3_600_000_000_000,
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
                    "acked": 239_990,
                    "unacked_at_exit": 0,
                    "connects": 1,
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
                    "negative_latency_count": 0, "above_highest_latency_count": 0, "clock_steps": 0,
                    "ignored_sequence_count": 0,
                    "latency_p50_ns": 100,
                    "latency_p95_ns": 200,
                    "latency_p99_ns": 300,
                    "histogram_lowest_ns": 1_000,
                    "histogram_highest_ns": 3_600_000_000_000,
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
            "acked": 239_990,
            "received_events": 239_985,
            "received_unique": 239_980,
            "downstream_lost": 10,
            "total_undelivered": 20,
            "duplicates": 5,
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
        "acked": enqueued,
        "received_events": enqueued,
        "received_unique": enqueued,
        "downstream_lost": 0,
        "total_undelivered": rejected,
        "duplicates": 0,
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


def test_source_state_uses_deploy_receipt_without_git_checkout() -> None:
    with tempfile.TemporaryDirectory() as tmp:
        root = Path(tmp)
        receipt = {
            "git_sha": "a" * 40,
            "git_dirty": False,
            "git_tags": ["rpi5-capacity-scout-v2"],
        }
        (root / "SOURCE_STATE.json").write_text(json.dumps(receipt))

        assert source_state(root) == receipt


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
    assert len(schedule) == matrix["final_campaign"]["expected_schedule_records"] == 2_201
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


def test_capacity_knee_schedule_is_exact_deterministic_counterbalanced_and_cooled() -> None:
    schedule = build_capacity_knee_schedule(seed=1729)
    expected_rates = {
        "mqtt-loopback": {*range(4_000, 16_000, 1_000), 15_250, 15_500, 15_750, 16_000},
        "native": set(range(8_000, 16_000, 1_000)),
        "wafer": set(range(8_000, 16_000, 1_000)),
        "ekuiper": set(range(4_000, 9_000, 1_000)),
    }

    assert len(schedule) == 5 * sum(map(len, expected_rates.values())) == 185
    assert schedule == build_capacity_knee_schedule(seed=1729)
    assert schedule != build_capacity_knee_schedule(seed=1730)
    assert all(item.experiment == "e-perf-capacity-knee" for item in schedule)
    assert all(item.loadgen_profile == "eval/loadgen/capacity-knee.toml" for item in schedule)
    assert all(item.total_messages == item.offered_rate_msg_s * 60 for item in schedule)
    assert all(item.exclusive_sut for item in schedule)
    for system, rates in expected_rates.items():
        observed = [item for item in schedule if item.system == system]
        assert {item.offered_rate_msg_s for item in observed} == rates
        assert {item.run_index for item in observed} == set(range(1, 6))
        assert len(observed) == 5 * len(rates)
    rate_positions = {
        rate: [] for rate in sorted(set().union(*expected_rates.values()))
    }
    for run_index in range(1, 6):
        rates = []
        for item in schedule:
            if item.run_index != run_index or item.offered_rate_msg_s in rates:
                continue
            rates.append(item.offered_rate_msg_s)
        assert rates != sorted(rates)
        assert rates != sorted(rates, reverse=True)
        for position, rate in enumerate(rates):
            rate_positions[rate].append(position)
    assert all(len(set(positions)) == 5 for positions in rate_positions.values())
    for rate in sorted(set().union(*expected_rates.values())):
        eligible = [system for system in runner.RATE_SWEEP_SYSTEMS if rate in expected_rates[system]]
        positions = {position: [] for position in range(len(eligible))}
        for run_index in range(1, 6):
            block = [
                item for item in schedule
                if item.run_index == run_index and item.offered_rate_msg_s == rate
            ]
            assert {item.system for item in block} == set(eligible)
            for position, item in enumerate(block):
                positions[position].append(item.system)
        for systems in positions.values():
            counts = [systems.count(system) for system in eligible]
            assert max(counts) - min(counts) <= 1
    assert runner.capacity_knee_cooldown_secs() == 60


def test_capacity_knee_schedule_rejects_grid_order_or_cooldown_drift(tmp_path: Path) -> None:
    matrix = json.loads((ROOT / "eval/canonical-matrix.json").read_text())
    definition = matrix["enhanced_candidate"]["experiments"]["e-perf-capacity-knee"]
    mutations = (
        lambda value: value["condition_grid_msg_s"]["mqtt-loopback"].pop(),
        lambda value: value["ordering"].update(method="monotonic ascending"),
        lambda value: value["ordering"].update(cooldown_secs=0),
        lambda value: value["delivery_good"].update(max_loss_percent=2.0),
    )
    for mutate in mutations:
        changed = json.loads(json.dumps(definition))
        mutate(changed)
        with pytest.raises(ValueError):
            runner.validate_capacity_knee_definition(changed)


def test_capacity_knee_cooldown_is_executed_and_auditable(tmp_path: Path) -> None:
    item = build_capacity_knee_schedule(seed=1729)[0]
    slept = []

    apply_capacity_knee_cooldown(tmp_path, item, sleep=slept.append)

    assert slept == [60]
    events = [json.loads(line) for line in (tmp_path / "progress.jsonl").read_text().splitlines()]
    assert [event["event"] for event in events] == ["cooldown-started", "cooldown-finished"]
    assert all(event["item"] == item.result_key for event in events)
    assert all(event["cooldown_secs"] == 60 for event in events)


def test_capacity_knee_cli_dry_run_exposes_candidate_plan() -> None:
    result = subprocess.run(
        [
            sys.executable,
            str(ROOT / "eval/scripts/lib/canonical_runner.py"),
            "--experiments",
            "e-perf-capacity-knee",
            "--batch-id",
            "test",
            "--seed",
            "1729",
            "--dry-run",
        ],
        cwd=ROOT,
        capture_output=True,
        text=True,
        check=False,
    )

    assert result.returncode == 0, result.stderr
    assert result.stdout.count("PLAN e-perf-capacity-knee") == 37
    assert "runs=5" in result.stdout
    assert "cooldown=60s" in result.stdout


def run_runner(*args: str) -> subprocess.CompletedProcess[str]:
    return subprocess.run(
        [sys.executable, str(ROOT / "eval/scripts/lib/canonical_runner.py"), *args],
        cwd=ROOT,
        capture_output=True,
        text=True,
        check=False,
    )


def test_repetitions_keeps_the_first_runs_of_the_frozen_schedule_and_marks_the_batch() -> None:
    full = build_schedule({"e-perf-1"}, seed=1729)

    result = run_runner(
        "--experiments", "e-perf-1", "--batch-id", "test", "--repetitions", "5", "--dry-run"
    )

    assert result.returncode == 0, result.stderr
    assert "DIAGNOSTIC repetitions=5: not thesis evidence" in result.stdout
    plans = [line for line in result.stdout.splitlines() if line.startswith("PLAN ")]
    assert len(plans) == len({item.condition for item in full})
    assert all(" runs=5 " in line for line in plans)


@pytest.mark.parametrize(
    "extra",
    [["--repetitions", "30"], ["--repetitions", "0"], ["--repetitions", "5", "--capacity-scout"]],
)
def test_repetitions_rejects_the_frozen_count_and_frozen_modes(extra: list[str]) -> None:
    arguments = ["--batch-id", "test", "--dry-run", *extra]
    if "--capacity-scout" not in extra:
        arguments = ["--experiments", "e-perf-1", *arguments]

    result = run_runner(*arguments)

    assert result.returncode == 2
    assert "--repetitions" in result.stderr


def test_diagnostic_leaves_are_not_thesis_evidence(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    monkeypatch.setenv(runner.DIAGNOSTIC_REPETITIONS_ENV, "5")
    (tmp_path / "metadata.json").write_text("{}")
    (tmp_path / "startup.json").write_text(json.dumps(_startup_artifact()))
    item = RunItem(
        experiment="e-perf-9",
        condition="small-warm",
        run_index=1,
        config="eval/configs/e-perf-9/pipeline-tier-small.toml",
        warmup_secs=0,
        measurement_secs=0,
        startup_mode="warm",
    )

    postprocess_run(ROOT, item, tmp_path)

    metadata = json.loads((tmp_path / "metadata.json").read_text())
    assert metadata["thesis_evidence"] is False
    assert metadata["diagnostic_repetitions"] == 5


def test_repetitions_drops_alias_views_of_measured_runs() -> None:
    result = run_runner(
        "--experiments", "e-perf-1,e-perf-2", "--batch-id", "test", "--repetitions", "5",
        "--dry-run",
    )

    assert result.returncode == 0, result.stderr
    assert "PLAN e-perf-1 " in result.stdout
    assert "PLAN e-perf-2 " not in result.stdout


def test_status_reports_done_and_pending_runs_and_resume_keeps_the_schedule(
    tmp_path: Path,
) -> None:
    results = tmp_path / "eval/results"
    ledger = results / "canonical-batches/rpi5-test"
    ledger.mkdir(parents=True)
    schedule = build_schedule({"e-perf-1"}, seed=1729)[:3]
    (ledger / "schedule.json").write_text(
        json.dumps([item.__dict__ for item in schedule], indent=2) + "\n"
    )
    (ledger / "progress.jsonl").write_text('{"event":"item-finished","completed":1}\n')
    first = schedule[0]
    attempt = results / first.experiment / "rpi5-test" / first.condition / "run-01-attempt-01"
    attempt.mkdir(parents=True)
    (attempt / "canonical-status.json").write_text('{"status":"passed"}')
    spent = schedule[2]
    for number in (1, 2):
        failed = (
            results / spent.experiment / "rpi5-test" / spent.condition
            / f"run-{spent.run_index:02d}-attempt-{number:02d}"
        )
        failed.mkdir(parents=True)
        (failed / "canonical-status.json").write_text(
            '{"status":"failed","failure_class":"infrastructure","reasons":["harness-error"]}'
        )
    environment = {key: value for key, value in os.environ.items() if key != "WAFER_RESULTS_ROOT"}

    def runner_in_tmp(*args: str) -> subprocess.CompletedProcess[str]:
        return subprocess.run(
            [
                sys.executable,
                str(ROOT / "eval/scripts/lib/canonical_runner.py"),
                "--root",
                str(tmp_path),
                "--batch-id",
                "test",
                *args,
            ],
            cwd=ROOT,
            capture_output=True,
            text=True,
            check=False,
            env=environment,
        )

    status = runner_in_tmp("--status")
    resumed = runner_in_tmp("--experiments", "e-perf-1", "--repetitions", "2", "--execute")

    assert status.returncode == 0, status.stderr
    assert "e-perf-1 1/3" in status.stdout
    assert "done 1/3, pending 1, missing 1" in status.stdout
    assert f"missing {spent.result_key}" in status.stdout
    assert f"next {schedule[1].result_key}" in status.stdout
    assert '"event":"item-finished"' in status.stdout
    assert resumed.returncode == 2
    assert "was started with a different schedule" in resumed.stderr


def test_capacity_knee_candidate_invocations_are_trace_free() -> None:
    item = next(
        item for item in build_capacity_knee_schedule(seed=1729)
        if item.system == "wafer" and item.offered_rate_msg_s == 9_000
    )
    receipt = runner.build_capacity_invocation(ROOT, item, Path("/tmp/capacity-knee"))

    assert receipt["publisher_command"][receipt["publisher_command"].index("--rate") + 1] == "9000"
    assert "--trace-file" not in receipt["publisher_command"]
    assert "--trace-file" not in receipt["subscriber_command"]
    assert item.config == "eval/configs/pipeline-a-wafer.toml"


def test_default_all_schedule_does_not_admit_candidate_experiments() -> None:
    assert runner.parse_experiments("all") == set(
        json.loads((ROOT / "eval/canonical-matrix.json").read_text())["experiments"]
    )
    assert "e-perf-capacity-knee" not in runner.parse_experiments("all")



def test_candidates_set_schedules_one_candidate_only_batch() -> None:
    experiments = runner.parse_experiments("candidates")

    assert experiments == runner.EXECUTABLE_CANDIDATE_EXPERIMENTS
    assert not experiments & runner.parse_experiments("all")
    assert {item.experiment for item in build_schedule(experiments, seed=1729)} == experiments
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
    assert subscriber[subscriber.index("--measurement-secs") + 1] == "60"
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
                "acked": enqueued,
                "received_events": received_unique,
                "received_unique": received_unique,
                "downstream_lost": downstream_lost,
                "total_undelivered": total_undelivered,
                "duplicates": 0,
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
                "highest_ns": 3_600_000_000_000,
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


def candidate_capacity_run_fixture(
    system: str,
    rate: int,
    *,
    run_index: int = 1,
    loss_ratio: float = 0.0,
    achieved_ratio: float | None = None,
    duplicates: int = 0,
) -> dict:
    result = capacity_run_fixture("wafer", 1_000, run_index=run_index)
    intended = rate * 60
    total_undelivered = round(intended * loss_ratio)
    received_unique = intended - total_undelivered
    received_events = received_unique + duplicates
    achieved_ratio = achieved_ratio if achieved_ratio is not None else received_unique / intended
    result.update(
        batch_class="candidate-capacity-knee",
        experiment="e-perf-capacity-knee",
        thesis_evidence=False,
        evidence_class="candidate-supplementary",
        n30_admitted=False,
        system=system,
        rate_msg_s=rate,
        messages={
            "intended": intended,
            "rejected": total_undelivered,
            "enqueued": received_unique,
            "acked": received_unique,
            "received_events": received_events,
            "received_unique": received_unique,
            "downstream_lost": 0,
            "total_undelivered": total_undelivered,
            "duplicates": duplicates,
            "ignored_warmup": 0,
        },
        rates_msg_s={
            "intended": float(rate),
            "achieved": rate * achieved_ratio,
            "achieved_ratio": achieved_ratio,
        },
        loss_percent=100.0 * loss_ratio,
    )
    result["latency_hdr"]["samples"] = received_events
    result["resources"]["scope"] = "no-sut" if system == "mqtt-loopback" else "sut"
    return result


def test_capacity_knee_estimator_uses_pooled_loss_ratio_and_zero_duplicates() -> None:
    runs = {
        system: [
            candidate_capacity_run_fixture(
                system,
                rate,
                run_index=run_index,
                loss_ratio=(0.03 if run_index in (4, 5) and rate == min(rates) else 0.0),
            )
            for rate in rates
            for run_index in range(1, 6)
        ]
        for system, rates in {
            "mqtt-loopback": [*range(4_000, 16_000, 1_000), 15_250, 15_500, 15_750, 16_000],
            "native": list(range(8_000, 16_000, 1_000)),
            "wafer": list(range(8_000, 16_000, 1_000)),
            "ekuiper": list(range(4_000, 9_000, 1_000)),
        }.items()
    }

    summary = estimate_candidate_capacity_envelope(runs)

    assert summary["experiment"] == "e-perf-capacity-knee"
    assert summary["thesis_evidence"] is False
    assert summary["n30_admitted"] is False
    assert summary["systems"]["mqtt-loopback"]["rates"][0]["pooled_loss"] == pytest.approx(0.012)
    assert summary["systems"]["mqtt-loopback"]["rates"][0]["classification"] == "bad"
    assert summary["systems"]["native"]["rates"][0]["classification"] == "support-confounded"

    runs["mqtt-loopback"] = [
        candidate_capacity_run_fixture(
            "mqtt-loopback", rate, run_index=run_index, duplicates=1 if rate == 4_000 and run_index == 1 else 0
        )
        for rate in [*range(4_000, 16_000, 1_000), 15_250, 15_500, 15_750, 16_000]
        for run_index in range(1, 6)
    ]
    summary = estimate_candidate_capacity_envelope(runs)
    assert summary["systems"]["mqtt-loopback"]["rates"][0]["classification"] == "bad"


def test_capacity_knee_summary_is_manifest_only_and_uses_passed_attempts(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    volume = tmp_path / "results"
    volume.mkdir()
    layout = runner.ResultsLayout.resolve(tmp_path, volume, mount_check=lambda _: True)
    layout.prepare()
    monkeypatch.setattr(runner, "results_layout", lambda _: layout)
    for system, rates in runner.CAPACITY_KNEE_GRID.items():
        for rate in rates:
            for run_index in range(1, 6):
                leaf = layout.raw_path(
                    "e-perf-capacity-knee",
                    "rpi5-test",
                    f"{system}/rate-{rate:05d}/run-{run_index:02d}-attempt-01",
                )
                leaf.mkdir(parents=True)
                (leaf / "capacity-run.json").write_text(
                    json.dumps(candidate_capacity_run_fixture(system, rate, run_index=run_index))
                )
                (leaf / "canonical-status.json").write_text('{"status":"passed"}')
    failed = layout.raw_path(
        "e-perf-capacity-knee",
        "rpi5-test",
        "wafer/rate-08000/run-01-attempt-02",
    )
    failed.mkdir(parents=True)
    (failed / "capacity-run.json").write_text(
        json.dumps(candidate_capacity_run_fixture("wafer", 8_000, run_index=1))
    )
    (failed / "canonical-status.json").write_text('{"status":"failed"}')

    path = summarize_capacity_knee(tmp_path, "test")
    summary = json.loads(path.read_text())

    assert path == volume / "manifests/candidate-batches/rpi5-test/capacity-knee-summary.json"
    assert summary["systems"]["wafer"]["complete"] is True
    assert all(row["run_count"] == 5 for row in summary["systems"]["wafer"]["rates"])
    assert not (layout.raw / "e-perf-capacity-knee/rpi5-test/capacity-knee-summary.json").exists()

    missing = layout.raw_path(
        "e-perf-capacity-knee",
        "rpi5-test",
        "wafer/rate-08000/run-05-attempt-01/canonical-status.json",
    )
    missing.write_text('{"status":"failed"}')
    incomplete = json.loads(summarize_capacity_knee(tmp_path, "test").read_text())
    assert incomplete["systems"]["wafer"]["complete"] is False


def test_candidate_capacity_result_rejects_evidence_promotion_and_threshold_drift() -> None:
    result = candidate_capacity_run_fixture("wafer", 9_000)
    validate_candidate_capacity_run_result(result)

    promoted = json.loads(json.dumps(result))
    promoted["thesis_evidence"] = True
    with pytest.raises(ValueError, match="thesis_evidence=false"):
        validate_candidate_capacity_run_result(promoted)

    duplicate = json.loads(json.dumps(result))
    duplicate["messages"]["duplicates"] = 1
    duplicate["messages"]["received_events"] += 1
    duplicate["latency_hdr"]["samples"] += 1
    validate_candidate_capacity_run_result(duplicate)
    assert estimate_candidate_capacity_envelope({
        system: [
            candidate_capacity_run_fixture(system, rate, run_index=index)
            for rate in runner.CAPACITY_KNEE_GRID[system]
            for index in range(1, 6)
        ]
        for system in runner.RATE_SWEEP_SYSTEMS
    })["criteria"] == {
        "loss_aggregation": "sum(total_undelivered) / sum(intended)",
        "max_pooled_loss": 0.01,
        "achieved_aggregation": "mean(run achieved_rate / intended_rate)",
        "min_mean_achieved_ratio": 0.99,
        "duplicates_allowed": 0,
        "support_path_censoring": "mqtt-loopback",
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


def test_capacity_estimator_rejects_duplicates_as_delivery_bad() -> None:
    rates = {
        rate: (0.0, 1.0, 1_000_000)
        for rate in (1000, 4000, 8000, 15000, 16000)
    }
    runs = capacity_batch(
        {system: rates for system in ("mqtt-loopback", "native", "wafer", "ekuiper")}
    )
    duplicate = runs["mqtt-loopback"][0]
    duplicate["messages"]["duplicates"] = 1
    duplicate["messages"]["received_events"] += 1
    duplicate["latency_hdr"]["samples"] += 1

    summary = estimate_capacity_envelope(runs)

    assert summary["systems"]["mqtt-loopback"]["rates"][0]["classification"] == "bad"


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
    first = summary["systems"]["wafer"]["rates"][0]
    assert len(first["run_summary"]["p99_ns"]["values"]) == 30
    assert len(first["normalized_p99"]["values"]) == 30
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


def test_capacity_estimator_normalizes_against_a_baseline_with_an_outcome_run() -> None:
    rates = {rate: (0.0, 1.0, 1_000_000) for rate in (1000, 4000, 8000, 15000, 16000)}
    runs = capacity_batch({system: rates for system in ("mqtt-loopback", "native", "wafer", "ekuiper")})
    runs["wafer"] = [
        run for run in runs["wafer"] if (run["rate_msg_s"], run["run_index"]) != (1000, 30)
    ]

    summary = estimate_capacity_envelope(runs, {("wafer", 1000): 1})

    wafer = summary["systems"]["wafer"]
    assert wafer["rates"][0]["classification"] == "bad"
    assert len(wafer["rates"][1]["normalized_p99"]["values"]) == 30


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


def test_rate_sweep_summary_counts_a_system_outcome_against_its_rate(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    volume = tmp_path / "results"
    volume.mkdir()
    layout = runner.ResultsLayout.resolve(tmp_path, volume, mount_check=lambda _: True)
    layout.prepare()
    monkeypatch.setattr(runner, "results_layout", lambda _: layout)
    layout.manifest_path("canonical-batches", "rpi5-test").mkdir(parents=True)
    crashed = ("wafer", runner.RATE_SWEEP_RATES[-1], 30)
    for system in runner.RATE_SWEEP_SYSTEMS:
        for rate in runner.RATE_SWEEP_RATES:
            for run_index in range(1, 31):
                leaf = layout.raw_path(
                    "e-perf-10", "rpi5-test", f"{system}/rate-{rate:05d}/run-{run_index:02d}"
                )
                leaf.mkdir(parents=True)
                if (system, rate, run_index) == crashed:
                    receipt = {
                        "status": "failed",
                        "failure_class": "sut_outcome",
                        "reasons": ["runtime-exit"],
                    }
                else:
                    (leaf / "capacity-run.json").write_text(
                        json.dumps(capacity_run_fixture(system, rate, run_index=run_index))
                    )
                    receipt = {"status": "passed"}
                (leaf / "canonical-status.json").write_text(json.dumps(receipt))

    summary = json.loads(summarize_rate_sweep(tmp_path, "test").read_text())

    wafer = summary["systems"]["wafer"]
    top = wafer["rates"][-1]
    assert (top["run_count"], top["sut_outcome_runs"], top["classification"]) == (29, 1, "bad")
    assert wafer["complete"] is True
    assert wafer["delivery_ceiling"] == {
        "rate_msg_s": runner.RATE_SWEEP_RATES[-2],
        "censoring": "none",
    }
    assert summary["systems"]["mqtt-loopback"]["delivery_ceiling"]["censoring"] == (
        f"right-censored-above-{runner.RATE_SWEEP_RATES[-1]}"
    )


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
    for experiment in ("e-swap-1", "e-swap-2", "e-swap-5", "e-swap-6"):
        items = by_experiment[experiment]
        assert [item.run_index for item in items] == list(range(1, 11)), experiment
        assert all(item.events_per_run == 50 for item in items), experiment
    assert len(by_experiment["e-swap-4"]) == 30
    assert all(item.events_per_run is None for item in by_experiment["e-swap-4"])
    assert all(item.shared_from is None for item in by_experiment["e-swap-1"] + by_experiment["e-swap-5"])
    assert all(item.shared_from == "e-swap-1" for item in by_experiment["e-swap-2"] + by_experiment["e-swap-6"])
    assert all("shakedown-macos" not in item.result_key for item in schedule)

    matrix = json.loads((ROOT / "eval/canonical-matrix.json").read_text())["experiments"]
    for index in range(1, 9):
        assert "per_node_metrics.csv" in matrix[f"e-iso-{index}"]["required_outputs"]
    iso_7_outputs = set(matrix["e-iso-7"]["required_outputs"])
    assert "branch-isolation.json" in iso_7_outputs
    assert not {"latency.hdr", "throughput.csv", "sequence.csv"} & iso_7_outputs
    assert {"recovery.csv", "recovery.json"} <= set(matrix["e-iso-8"]["required_outputs"])
    for index in (1, 2, 4, 6):
        assert "swap_timeline.json" in matrix[f"e-swap-{index}"]["required_outputs"]
    assert "swap_timeline.json" not in matrix["e-swap-3"]["required_outputs"]
    for index in (1, 2, 4, 6):
        outputs = set(matrix[f"e-swap-{index}"]["required_outputs"])
        assert {"swap_requests.json", "hotswap-analysis.json"} <= outputs
    rollback_outputs = set(matrix["e-swap-5"]["required_outputs"])
    assert "swap_timeline.json" not in rollback_outputs
    assert {
        "swap_requests.json",
        "rollback.json",
        "post-rollback-continuity.json",
        "sequence.csv",
    } <= rollback_outputs


def test_swap3_final_artifact_sets_match_matrix_runner_verifier_and_analysis() -> None:
    matrix_outputs = set(
        json.loads((ROOT / "eval/canonical-matrix.json").read_text())["experiments"][
            "e-swap-3"
        ]["required_outputs"]
    )
    item = next(
        candidate
        for candidate in build_schedule({"e-swap-3"}, seed=1729)
        if candidate.condition == "wafer-hotswap" and candidate.run_index == 1
    )
    invocation = build_swap3_invocation(ROOT, item, Path("/tmp/e-swap-3"))
    producer_outputs = {
        "latency.hdr",
        "throughput.csv",
        "sequence.csv",
        "subscriber-metadata.json",
        "throughput-buckets.json",
        "throughput-buckets-10ms.json",
        invocation["controlled_factors"]["publisher_summary"],
        invocation["controlled_factors"]["action_timing_receipt"],
        "disruption-analysis.json",
    }
    verifier_outputs = {
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
    analysis_outputs = {
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

    assert matrix_outputs == producer_outputs
    assert matrix_outputs == verifier_outputs
    assert matrix_outputs == analysis_outputs


def test_isolation_derivations_use_raw_runtime_metrics() -> None:
    with tempfile.TemporaryDirectory() as tmp:
        root = Path(tmp)
        write_node_metrics(root, attack={
            "messages_in": 12_000,
            "traps_total": 12_000,
            "traps_unreachable": 12_000,
            "attempts_failed": 12_000,
            "dropped_on_recovery": 12_000,
            "recovery_count": 12_000,
        })
        (root / "stdout.log").write_text("expected guest traps only\n")
        containment = derive_containment(root, "e-iso-6", "panic")
        assert containment["contained"] is True
        assert containment["expected_mechanism"] == "traps_unreachable"
        assert containment["traps_total"] == 12_000
        assert containment["runtime_panic"] is False
        assert containment["dlq_sent_total"] == 0
        assert containment["dlq_records"] is None, "no dead-letter file was written"

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
                "total_expected,total_received,received_unique,gap_ranges,gap_msgs,duplicates_count\n"
                f"{received},{received},{received},0,0,0\n"
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
                "total_expected,total_received,received_unique,gap_ranges,gap_msgs,duplicates_count\n"
                "28000,28000,28000,0,0,0\n"
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


def test_branch_isolation_loss_does_not_count_duplicates_as_received() -> None:
    with tempfile.TemporaryDirectory() as tmp:
        root = Path(tmp)
        for branch in ("branch-a", "branch-b"):
            branch_dir = root / branch
            branch_dir.mkdir()
            branch_dir.joinpath("throughput.csv").write_text("elapsed_secs,msg_count,bytes\n60.0,60000,7680000\n")
            branch_dir.joinpath("sequence.csv").write_text(
                "total_expected,total_received,received_unique,gap_ranges,gap_msgs,duplicates_count\n"
                "60000,60000,59995,1,5,5\n"
            )
            branch_dir.joinpath("percentiles.json").write_text(
                json.dumps({"total_count": 60_000, "p50_ns": 1, "p95_ns": 2, "p99_ns": 3, "p999_ns": 4})
            )
            branch_dir.joinpath("measurement-window.json").write_text(
                json.dumps({"started_ns": 1_000_000_000, "finished_ns": 61_000_000_000})
            )

        branch_a = derive_branch_isolation(
            root,
            warmup_secs=30,
            measurement_secs=60,
            branch_sources={"branch-a": "source_a", "branch-b": "source_b"},
            target_messages=60_000,
        )["branches"]["branch_a"]
        assert branch_a["received_messages"] == 60_000
        assert branch_a["received_unique_messages"] == 59_995
        assert branch_a["lost_messages"] == 5
        assert branch_a["duplicates"] == 5


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


NODE_METRIC_COLUMNS = (
    "node_id,messages_in,messages_out,filtered_out,traps_total,traps_memory_out_of_bounds,"
    "traps_unreachable,traps_interrupt,traps_out_of_fuel,traps_memory_limit,traps_other,"
    "guest_bad_input,guest_dependency_failed,guest_processing_failed,guest_timed_out,"
    "guest_unrecoverable,attempts_failed,retries,dlq_sent,dlq_lost,skipped,"
    "retry_exhausted_skips,dropped_on_recovery,dropped_on_teardown,error_state_seconds,recovery_count"
).split(",")


def write_node_metrics(root: Path, attack: dict[str, int]) -> None:
    def row(node_id: str, values: dict[str, int]) -> str:
        return ",".join([node_id] + [str(values.get(column, 0)) for column in NODE_METRIC_COLUMNS[1:]])

    (root / "per_node_metrics.csv").write_text("\n".join([
        ",".join(NODE_METRIC_COLUMNS),
        row("source", {"messages_out": 12_000}),
        row("attack", attack),
        row("sink", {}),
    ]) + "\n")


@pytest.mark.parametrize(
    ("experiment", "condition", "attack", "contained"),
    [
        ("e-iso-3", "fs-access", {"messages_in": 10, "guest_unrecoverable": 10, "attempts_failed": 10}, True),
        ("e-iso-3", "fs-access", {"messages_in": 10, "guest_processing_failed": 10, "attempts_failed": 10}, False),
        ("e-iso-6", "panic", {"messages_in": 10, "guest_bad_input": 10, "attempts_failed": 10}, False),
        ("e-iso-4", "infinite-loop", {"messages_in": 10, "traps_total": 10, "traps_out_of_fuel": 10, "attempts_failed": 10}, False),
        ("e-iso-1", "buffer-overflow", {"messages_in": 10, "messages_out": 1, "traps_total": 9, "traps_memory_out_of_bounds": 9, "attempts_failed": 9}, False),
        ("e-iso-7", "control", {}, None),
    ],
)
def test_containment_requires_the_expected_mechanism_alone(
    experiment: str, condition: str, attack: dict[str, int], contained: bool | None
) -> None:
    with tempfile.TemporaryDirectory() as tmp:
        root = Path(tmp)
        write_node_metrics(root, attack)
        (root / "stdout.log").write_text("")
        assert derive_containment(root, experiment, condition)["contained"] is contained


def test_containment_reports_dead_letter_evidence() -> None:
    with tempfile.TemporaryDirectory() as tmp:
        root = Path(tmp)
        write_node_metrics(root, attack={
            "messages_in": 3,
            "traps_total": 3,
            "traps_memory_out_of_bounds": 3,
            "attempts_failed": 3,
            "dlq_sent": 3,
            "recovery_count": 3,
        })
        (root / "stdout.log").write_text("")
        (root / "dlq.jsonl").write_text(
            '{"reason":{"type":"trapped","kind":"memory_out_of_bounds"}}\n' * 3 + "\n"
        )
        containment = derive_containment(root, "e-iso-1", "buffer-overflow")
        assert containment["contained"] is True
        assert containment["dlq_sent_total"] == 3
        assert containment["dlq_records"] == 3, "blank lines are not records"


def test_containment_rejects_metrics_without_split_counters() -> None:
    with tempfile.TemporaryDirectory() as tmp:
        root = Path(tmp)
        (root / "per_node_metrics.csv").write_text(
            "node_id,messages_in,messages_out,traps_total,error_state_seconds,recovery_count\n"
            "attack,12000,0,12000,1.2,12000\n"
        )
        (root / "stdout.log").write_text("")
        with pytest.raises(ValueError, match="invalid runtime metrics"):
            derive_containment(root, "e-iso-6", "panic")


def test_containment_derivation_rejects_placeholder_metrics() -> None:
    with tempfile.TemporaryDirectory() as tmp:
        root = Path(tmp)
        (root / "per_node_metrics.csv").write_text(
            "node_id,messages_in,messages_out,traps_total,error_state_seconds,recovery_count\n"
            "# runtime did not emit final metrics\n"
        )
        (root / "stdout.log").write_text("runtime required SIGKILL\n")
        with pytest.raises(ValueError, match="no runtime metric rows"):
            derive_containment(root, "e-iso-6", "panic")


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
    assert f"version: {comparator['version']}\n" in install
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
        (output / "interval-latency.json").write_text(json.dumps({
            "schema_version": 1,
            "interval_clock": "monotonic-elapsed",
            "alignment_clock": "unix-epoch",
            "alignment_clock_purpose": "cross-process-alignment-only",
            "measurement_start_unix_epoch_ns": 1_000_000_000,
            "declared_measurement_duration_ns": 60_000_000_000,
            "bucket_width_ns": 1_000_000_000,
            "maximum_rows": 62,
            "row_count": 1,
            "aggregate_latency_count": 60_000,
            "late_arrivals": 0,
            "rows": [{
                "interval_start_ns": 0,
                "interval_end_ns": 1_000_000_000,
                "interval_start_unix_epoch_ns": 1_000_000_000,
                "interval_end_unix_epoch_ns": 2_000_000_000,
                "latency_count": 60_000,
                "latency_p50_ns": 1_000,
                "latency_p95_ns": 2_000,
                "latency_p99_ns": 3_000,
                "received_events": 60_000,
                "throughput_messages": 60_000,
                "duplicates": 0,
            }],
        }))
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


def test_resume_uses_next_numeric_attempt_after_a_gap() -> None:
    with tempfile.TemporaryDirectory() as tmp:
        condition = Path(tmp)
        (condition / "run-01-attempt-01").mkdir()
        (condition / "run-01-attempt-03").mkdir()

        selection = select_attempt(condition, 1)

        assert selection.skip is False
        assert selection.path.name == "run-01-attempt-04"


RUNTIME_EXIT_RECEIPT = {
    "status": "failed",
    "failure_class": "sut_outcome",
    "reasons": ["runtime-exit"],
}


@pytest.mark.parametrize("first", [{"status": "passed"}, RUNTIME_EXIT_RECEIPT])
def test_resume_rejects_multiple_admitted_attempts(first: dict) -> None:
    with tempfile.TemporaryDirectory() as tmp:
        condition = Path(tmp)
        for attempt, receipt in ((1, first), (2, {"status": "passed"})):
            path = condition / f"run-01-attempt-{attempt:02d}"
            path.mkdir()
            (path / "canonical-status.json").write_text(json.dumps(receipt))

        with pytest.raises(ValueError, match="multiple admitted attempts"):
            select_attempt(condition, 1)


def test_resume_rejects_linked_attempt_or_terminal_receipt() -> None:
    with tempfile.TemporaryDirectory() as tmp:
        condition = Path(tmp)
        outside = condition / "outside"
        outside.mkdir()
        linked_attempt = condition / "run-01-attempt-01"
        linked_attempt.symlink_to(outside, target_is_directory=True)
        with pytest.raises(ValueError, match="malformed attempt path"):
            select_attempt(condition, 1)

        linked_attempt.unlink()
        attempt = condition / "run-01-attempt-01"
        attempt.mkdir()
        target = condition / "status.json"
        target.write_text('{"status":"passed"}')
        (attempt / "canonical-status.json").symlink_to(target)
        with pytest.raises(ValueError, match="linked terminal receipt"):
            select_attempt(condition, 1)


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


def perf_item() -> RunItem:
    return next(
        item
        for item in build_schedule({"e-perf-3"}, seed=1729)
        if item.condition == "depth-1" and item.run_index == 1
    )


def receipts(condition: Path) -> list[dict | None]:
    return [
        json.loads((path / "canonical-status.json").read_text())
        if (path / "canonical-status.json").is_file()
        else None
        for path in sorted(condition.iterdir())
    ]


@pytest.fixture
def generic_runner(monkeypatch: pytest.MonkeyPatch) -> list[list[str]]:
    """Run the generic path without hardware; each run-experiment.sh launch is recorded."""
    monkeypatch.delenv("WAFER_RESULTS_ROOT", raising=False)
    monkeypatch.delenv("WAFER_DIAGNOSTIC_REPETITIONS", raising=False)
    monkeypatch.setattr(runner, "set_ekuiper_active", lambda root, active: None)
    return []


def fail_launches(monkeypatch: pytest.MonkeyPatch, launches: list, failures: int) -> None:
    def launch(command, **kwargs):
        launches.append(command)
        if len(launches) <= failures:
            raise subprocess.CalledProcessError(4, command)

    monkeypatch.setattr(runner.subprocess, "run", launch)
    monkeypatch.setattr(runner, "postprocess_run", lambda root, item, output: None)
    monkeypatch.setattr(runner, "verify_result", lambda root, output: None)


def test_infrastructure_failure_is_retried_once_in_place_then_missing(
    monkeypatch: pytest.MonkeyPatch,
    tmp_path: Path,
    capsys: pytest.CaptureFixture[str],
    generic_runner: list,
) -> None:
    fail_launches(monkeypatch, generic_runner, failures=3)
    item = perf_item()

    assert runner.run_item(tmp_path, "test", item) is False
    assert runner.run_item(tmp_path, "test", item) is False

    condition = tmp_path / "eval/results/e-perf-3/rpi5-test/depth-1"
    assert [path.name for path in sorted(condition.iterdir())] == [
        "run-01-attempt-01",
        "run-01-attempt-02",
    ]
    assert [receipt["failure_class"] for receipt in receipts(condition)] == [
        "infrastructure",
        "infrastructure",
    ]
    assert receipts(condition)[0]["reasons"] == ["harness-error"]
    assert len(generic_runner) == 2
    assert "MISSING e-perf-3/depth-1/run-01: infrastructure retries spent" in (
        capsys.readouterr().out
    )


def test_infrastructure_retry_that_passes_is_the_admitted_attempt(
    monkeypatch: pytest.MonkeyPatch, tmp_path: Path, generic_runner: list
) -> None:
    fail_launches(monkeypatch, generic_runner, failures=1)
    item = perf_item()

    assert runner.run_item(tmp_path, "test", item)
    assert runner.run_item(tmp_path, "test", item)

    condition = tmp_path / "eval/results/e-perf-3/rpi5-test/depth-1"
    assert [receipt["status"] for receipt in receipts(condition)] == ["failed", "passed"]
    assert len(generic_runner) == 2


def test_interrupted_attempt_counts_as_one_infrastructure_attempt(
    monkeypatch: pytest.MonkeyPatch, tmp_path: Path, generic_runner: list
) -> None:
    fail_launches(monkeypatch, generic_runner, failures=3)
    item = perf_item()
    condition = tmp_path / "eval/results/e-perf-3/rpi5-test/depth-1"
    interrupted = condition / "run-01-attempt-01"
    interrupted.mkdir(parents=True)
    (interrupted / "stdout.log").write_text("power lost\n")

    assert runner.run_item(tmp_path, "test", item) is False

    assert receipts(condition)[0] is None
    assert receipts(condition)[1]["failure_class"] == "infrastructure"
    assert len(generic_runner) == 1


def test_validation_gate_unit_is_never_retried(
    monkeypatch: pytest.MonkeyPatch, tmp_path: Path, generic_runner: list
) -> None:
    fail_launches(monkeypatch, generic_runner, failures=3)
    item = next(iter(build_schedule({"e-val-1"}, seed=1729)))

    assert runner.run_item(tmp_path, "test", item) is False

    assert len(generic_runner) == 1
    condition = tmp_path / f"eval/results/e-val-1/rpi5-test/{item.condition}"
    assert [path.name for path in condition.iterdir()] == [f"run-{item.run_index:02d}-attempt-01"]


def write_stopped_runtime_leaf(output: Path, experiment: str, runtime_exit: int) -> None:
    """What run-experiment.sh leaves when the runtime exits before the run finishes."""
    output.mkdir(parents=True)
    for name, content in {
        "config.toml": "fixture\n",
        "stdout.log": "fixture\n",
        "runtime-provenance.json": "fixture\n",
        "pmic-rails.csv": "fixture\n",
        "power-boundary.json": '{"backend":"pi","measurement":"rpi5-pmic-internal-rail-proxy"}\n',
        "pi-telemetry.csv": (
            "timestamp_ns,temperature_millicelsius,cpu_frequency_hz,governor,throttled,"
            "rail_proxy_watts\n100,50000,2400000000,performance,0x0,4.0\n"
        ),
        "measurement-window.json": '{"started_ns":100,"finished_ns":200}\n',
    }.items():
        (output / name).write_text(content)
    metadata = {
        "experiment": experiment,
        "host_tag": "rpi5",
        "hardware_model": "Raspberry Pi 5 Model B Rev 1.0",
        "arch": "aarch64",
        "isolated_cpus": "",
        "housekeeping_cpus": "0",
        "irq_default_cpus": "0",
        "cpu_governors": ["performance"],
        "throttled": "0x0",
        "git_sha": "1" * 40,
        "git_dirty": False,
        "git_tags": ["rpi5-eval-v1"],
        "wasmtime_version": "43.0.0",
        "wafer_runtime_sha256": "2" * 64,
        "wafer_plugin_hashes": {"transform": "3" * 64},
        "engine_fuel_budgets": {"transform": 10_000_000, "filter": 500_000, "router": 500_000},
        "epoch_deadline": 100,
        "epoch_tick_ms": 10,
        "effective_metering_mode": "fuel-and-epoch",
        "exit_codes": {"wafer_runtime": runtime_exit},
    }
    (output / "metadata.json").write_text(json.dumps(metadata))


def test_forced_runtime_exit_in_containment_run_is_an_admitted_outcome(
    monkeypatch: pytest.MonkeyPatch,
    tmp_path: Path,
    capsys: pytest.CaptureFixture[str],
    generic_runner: list,
) -> None:
    (tmp_path / "eval").mkdir()
    (tmp_path / "eval/scripts").symlink_to(ROOT / "eval/scripts")
    item = next(item for item in build_schedule({"e-iso-6"}, seed=1729) if item.run_index == 1)
    real_run = subprocess.run

    def launch(command, **kwargs):
        if not str(command[0]).endswith("run-experiment.sh"):
            return real_run(command, **kwargs)
        generic_runner.append(command)
        output = Path(command[command.index("--output-dir") + 1])
        write_stopped_runtime_leaf(output, "e-iso-6", runtime_exit=134)
        raise subprocess.CalledProcessError(5, command)

    monkeypatch.setattr(runner.subprocess, "run", launch)

    assert runner.run_item(tmp_path, "test", item)
    assert runner.run_item(tmp_path, "test", item)

    condition = tmp_path / "eval/results/e-iso-6/rpi5-test/panic"
    assert receipts(condition) == [
        {
            "status": "failed",
            "experiment": "e-iso-6",
            "condition": "panic",
            "run_index": 1,
            "updated_at": receipts(condition)[0]["updated_at"],
            "failure_class": "sut_outcome",
            "reasons": ["runtime-exit"],
        }
    ]
    assert len(generic_runner) == 1
    output = capsys.readouterr().out
    assert "OUTCOME e-iso-6/panic/run-01: runtime-exit" in output
    assert "SKIP e-iso-6/panic/run-01" in output


def test_runtime_startup_refusal_stays_an_infrastructure_failure(
    monkeypatch: pytest.MonkeyPatch, tmp_path: Path, generic_runner: list
) -> None:
    (tmp_path / "eval").mkdir()
    (tmp_path / "eval/scripts").symlink_to(ROOT / "eval/scripts")
    item = next(item for item in build_schedule({"e-iso-6"}, seed=1729) if item.run_index == 1)
    real_run = subprocess.run

    def launch(command, **kwargs):
        if not str(command[0]).endswith("run-experiment.sh"):
            return real_run(command, **kwargs)
        generic_runner.append(command)
        output = Path(command[command.index("--output-dir") + 1])
        write_stopped_runtime_leaf(output, "e-iso-6", runtime_exit=2)
        raise subprocess.CalledProcessError(5, command)

    monkeypatch.setattr(runner.subprocess, "run", launch)

    assert runner.run_item(tmp_path, "test", item) is False

    condition = tmp_path / "eval/results/e-iso-6/rpi5-test/panic"
    assert [receipt["failure_class"] for receipt in receipts(condition)] == [
        "infrastructure",
        "infrastructure",
    ]
    assert len(generic_runner) == 2


class StoppedRuntime:
    """A runtime process that has already exited with this code."""

    def __init__(self, returncode: int) -> None:
        self.returncode = returncode
        self.pid = 4242

    def poll(self) -> int:
        return self.returncode

    def wait(self, timeout: float | None = None) -> int:
        return self.returncode

    def terminate(self) -> None:
        pass


def stub_runner_host(monkeypatch: pytest.MonkeyPatch, runtime_exit: int) -> None:
    """Stand in for the host gates, telemetry and the runtime of a runner-driven item."""

    def fake_run(command, **kwargs):
        if "--output" in command and "validate-canonical.py" in " ".join(map(str, command)):
            Path(command[command.index("--output") + 1]).write_text(
                json.dumps({"git_sha": "1" * 40})
            )
        return subprocess.CompletedProcess(command, 0)

    def fake_merge(path, *args):
        Path(path).write_text(json.dumps({"exit_codes": {"wafer_runtime": int(args[9])}}))

    monkeypatch.setattr(runner.subprocess, "run", fake_run)
    monkeypatch.setattr(
        runner.subprocess, "Popen", lambda command, **kwargs: StoppedRuntime(runtime_exit)
    )
    monkeypatch.setattr(runner, "merge_metadata", fake_merge)
    monkeypatch.setattr(runner, "set_ekuiper_active", lambda root, active: None)
    monkeypatch.setattr(runner, "start_pi_telemetry", lambda root, destination, item=None: [])
    monkeypatch.setattr(runner, "stop_pi_telemetry", lambda telemetry: None)
    monkeypatch.setattr(runner, "postprocess_run", lambda root, item, output: None)
    monkeypatch.setattr(runner, "verify_result", lambda root, output: None)
    monkeypatch.setattr(runner.time, "sleep", lambda seconds: None)


def test_rate_sweep_runtime_that_dies_at_startup_is_a_bounded_outcome(
    monkeypatch: pytest.MonkeyPatch, tmp_path: Path
) -> None:
    stub_runner_host(monkeypatch, runtime_exit=134)
    monkeypatch.setattr(runner, "_running_sut_processes", lambda: [])
    item = next(item for item in build_schedule({"e-perf-10"}, seed=1729) if item.system == "wafer")
    output = tmp_path / "wafer/rate-01000/run-01-attempt-01"

    assert runner.run_rate_sweep_item(ROOT, item, runner.AttemptSelection(output, False))

    receipt = json.loads((output / "canonical-status.json").read_text())
    assert (receipt["failure_class"], receipt["reasons"]) == ("sut_outcome", ["runtime-exit"])
    window = json.loads((output / "measurement-window.json").read_text())
    assert window["started_ns"] < window["finished_ns"]


def test_hot_swap_run_the_runtime_did_not_survive_keeps_its_window(
    monkeypatch: pytest.MonkeyPatch, tmp_path: Path
) -> None:
    stub_runner_host(monkeypatch, runtime_exit=134)
    monkeypatch.setattr(runner, "wait_for_api", lambda url: None)
    monkeypatch.setattr(runner, "post_hot_swap", lambda node, plugin: {"http_status": 200, "body": {}})
    item = next(iter(build_schedule({"e-swap-1"}, seed=1729)))
    output = tmp_path / "steady/run-01-attempt-01"

    assert runner.run_hot_swap_item(ROOT, item, runner.AttemptSelection(output, False))

    receipt = json.loads((output / "canonical-status.json").read_text())
    assert (receipt["failure_class"], receipt["reasons"]) == ("sut_outcome", ["runtime-exit"])
    window = json.loads((output / "measurement-window.json").read_text())
    assert window["started_ns"] < window["finished_ns"]


@pytest.mark.parametrize(
    ("runtime_exit", "failure_class", "reasons"),
    [(134, "sut_outcome", ["runtime-exit"]), (2, "infrastructure", ["harness-error"])],
)
def test_hot_swap_runtime_that_dies_before_its_control_plane_answers(
    monkeypatch: pytest.MonkeyPatch,
    tmp_path: Path,
    runtime_exit: int,
    failure_class: str,
    reasons: list[str],
) -> None:
    stub_runner_host(monkeypatch, runtime_exit=runtime_exit)

    def unanswered(url: str, timeout_secs: float = 10.0) -> None:
        raise RuntimeError(f"{url} did not answer")

    monkeypatch.setattr(runner, "wait_for_api", unanswered)
    item = next(iter(build_schedule({"e-swap-1"}, seed=1729)))
    output = tmp_path / "steady/run-01-attempt-01"

    admitted = runner.run_hot_swap_item(ROOT, item, runner.AttemptSelection(output, False))

    receipt = json.loads((output / "canonical-status.json").read_text())
    assert (admitted, receipt["failure_class"], receipt["reasons"]) == (
        failure_class == "sut_outcome",
        failure_class,
        reasons,
    )


@pytest.mark.parametrize(
    ("runtime_exit", "failure_class", "reasons"),
    [(134, "sut_outcome", ["runtime-exit"]), (2, "infrastructure", ["harness-error"])],
)
def test_disruption_runtime_that_dies_at_startup_is_judged_by_its_exit(
    monkeypatch: pytest.MonkeyPatch,
    tmp_path: Path,
    runtime_exit: int,
    failure_class: str,
    reasons: list[str],
) -> None:
    stub_runner_host(monkeypatch, runtime_exit=runtime_exit)
    item = next(
        item for item in build_schedule({"e-swap-3"}, seed=1729) if item.condition == "wafer-restart"
    )
    output = tmp_path / "wafer-restart/run-01-attempt-01"

    admitted = runner.run_restart_item(ROOT, item, runner.AttemptSelection(output, False))

    receipt = json.loads((output / "canonical-status.json").read_text())
    assert (admitted, receipt["failure_class"], receipt["reasons"]) == (
        failure_class == "sut_outcome",
        failure_class,
        reasons,
    )


def test_disruption_hot_swap_request_that_fails_is_a_failed_swap(
    monkeypatch: pytest.MonkeyPatch, tmp_path: Path
) -> None:
    stub_runner_host(monkeypatch, runtime_exit=0)
    clock = iter(range(1_000_000_000_000, 2_000_000_000_000, 1_000))
    monkeypatch.setattr(runner.time, "time_ns", lambda: next(clock))
    item = next(
        item for item in build_schedule({"e-swap-3"}, seed=1729) if item.condition == "wafer-hotswap"
    )
    output = tmp_path / "wafer-hotswap/run-01-attempt-01"

    class Process:
        returncode: int | None = None

        def poll(self) -> int | None:
            return self.returncode

        def wait(self, timeout: float | None = None) -> int:
            self.returncode = 0
            return 0

        def terminate(self) -> None:
            self.returncode = 0

    def launch(command: list[str], **kwargs: object) -> Process:
        if command == ["publish"]:
            now = next(clock)
            (output / "publisher-timing.json").write_text(
                json.dumps(
                    {
                        "measurement_started_unix_epoch_ns": now,
                        "event_unix_epoch_ns": now,
                        "event_offset_ns": runner.SWAP3_EVENT_OFFSET_NS,
                    }
                )
            )
        return Process()

    monkeypatch.setattr(runner.subprocess, "Popen", launch)
    monkeypatch.setattr(runner, "loadgen_command", lambda root, candidate, action, **kwargs: [action])
    monkeypatch.setattr(
        runner, "post_hot_swap", lambda node, plugin: {"http_status": 500, "body": "rejected"}
    )
    monkeypatch.setattr(runner, "wait_for_subscriber", lambda process, timeout=30: 0)

    assert runner.run_restart_item(ROOT, item, runner.AttemptSelection(output, False))

    receipt = json.loads((output / "canonical-status.json").read_text())
    assert (receipt["failure_class"], receipt["reasons"]) == ("sut_outcome", ["swap-failed"])
    assert json.loads((output / "swap_requests.json").read_text())[0]["http_status"] == 500


def test_summaries_continue_past_one_a_system_outcome_left_incomplete(
    monkeypatch: pytest.MonkeyPatch, capsys: pytest.CaptureFixture[str], tmp_path: Path
) -> None:
    written: list[str] = []

    def incomplete(root: Path, batch_id: str) -> None:
        raise ValueError("wafer rate 16000 lacks a run")

    monkeypatch.setattr(runner, "summarize_rate_sweep", incomplete)
    monkeypatch.setattr(
        runner, "summarize_branch_isolation", lambda root, batch_id: written.append("e-iso-7")
    )
    monkeypatch.setattr(runner, "summarize_swap4", lambda root, batch_id: written.append("e-swap-4"))

    runner.summarise(tmp_path, "test", {"e-perf-10", "e-iso-7", "e-swap-4"})

    assert written == ["e-iso-7", "e-swap-4"]
    assert "SUMMARY e-perf-10 incomplete: wafer rate 16000 lacks a run" in capsys.readouterr().out


def test_one_dropped_message_in_a_burst_swap_is_an_admitted_outcome(tmp_path: Path) -> None:
    timing, source, requests, sink, throughput, sequence = swap4_fixture()
    for bucket in throughput["drain_buckets"][:1]:
        bucket.update(received_unique=0, received_events=0, rate_msg_s=0)
    throughput.update(
        drain_received_unique=0,
        drain_received_events=0,
        drain_first_offset_ns=None,
        drain_last_offset_ns=None,
        max_arrival_offset_ns=throughput["primary_last_offset_ns"],
        received_unique=129_999,
        received_events=129_999,
        phase_received_messages=[55_000, 20_000, 54_999],
    )
    sequence.update(total_received="129999", gap_msgs="1")
    timeline = build_swap4_timeline(timing, source, requests, sink, throughput, sequence)
    validate_swap4_artifacts(timeline, throughput, requests, sink)
    assert timeline["loss"] == 1

    item = next(iter(build_schedule({"e-swap-4"}, seed=1729)))
    output = tmp_path / "burst-2x/run-01-attempt-01"
    output.mkdir(parents=True)
    (output / "burst-timeline.json").write_text(json.dumps(timeline))
    (output / "sequence.csv").write_text(
        "total_expected,total_received,gap_ranges,gap_msgs,duplicates_count\n"
        "130000,129999,129999-129999,1,0\n"
    )

    assert runner.finish_attempt(output, item)

    receipt = json.loads((output / "canonical-status.json").read_text())
    assert receipt["failure_class"] == "sut_outcome"
    assert receipt["reasons"] == ["message-loss"]
    assert select_attempt(output.parent, 1).skip is True


@pytest.mark.parametrize(
    ("strategy", "reasons"), [("wafer-hotswap", ["message-loss"]), ("wafer-restart", None)]
)
def test_disruption_loss_is_an_outcome_only_where_zero_loss_is_required(
    tmp_path: Path, strategy: str, reasons: list[str] | None
) -> None:
    item = next(item for item in build_schedule({"e-swap-3"}, seed=1729) if item.condition == strategy)
    output = tmp_path / strategy / "run-01-attempt-01"
    output.mkdir(parents=True)
    (output / "disruption-analysis.json").write_text(
        json.dumps({"strategy": strategy, "loss": 1, "duplicates": 0})
    )

    assert runner.finish_attempt(output, item)

    receipt = json.loads((output / "canonical-status.json").read_text())
    assert receipt.get("reasons") == reasons


def test_hot_swap_request_to_a_dead_runtime_is_a_failed_swap(
    monkeypatch: pytest.MonkeyPatch, tmp_path: Path
) -> None:
    def refused(request, timeout):
        raise runner.urllib.error.URLError(ConnectionRefusedError(111, "Connection refused"))

    monkeypatch.setattr(runner.urllib.request, "urlopen", refused)

    response = runner.post_hot_swap("transform", tmp_path / "v2.wasm")

    assert response["http_status"] is None
    assert "Connection refused" in response["body"]
    item = next(iter(build_schedule({"e-swap-1"}, seed=1729)))
    output = tmp_path / "steady/run-01-attempt-01"
    output.mkdir(parents=True)
    (output / "swap_requests.json").write_text(json.dumps([{"event_index": 0, **response}]))
    assert runner.incomplete_run(output, item)


def test_validation_gate_rejects_a_repetition_that_needed_a_retry() -> None:
    with tempfile.TemporaryDirectory() as tmp:
        root = Path(tmp)
        failed = root / "run-01-attempt-01"
        failed.mkdir()
        (failed / "canonical-status.json").write_text(
            json.dumps({"status": "failed", "failure_class": "infrastructure"})
        )
        retried = root / "run-01-attempt-02"
        retried.mkdir()
        (retried / "percentiles.json").write_text(
            json.dumps({"total_count": 600, "p99_ns": 51_000_000})
        )
        (retried / "canonical-status.json").write_text(json.dumps({"status": "passed"}))

        result = evaluate_validation_gate(root, expected_runs=1)

    assert result.passed is False
    assert result.failed_runs == [1]


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


SOURCE_SHA = "a" * 40


def run_main(
    monkeypatch: pytest.MonkeyPatch, capsys: pytest.CaptureFixture[str], *args: str
) -> tuple[int, str, str]:
    monkeypatch.setattr(sys, "argv", ["canonical_runner.py", *args])
    code = runner.main()
    captured = capsys.readouterr()
    return code, captured.out, captured.err


def passed_item(root: Path, batch_id: str, item: RunItem) -> bool:
    """Stand in for a real run: leave a passed, clean leaf or an alias receipt."""
    if item.shared_from:
        copy_shared_result(root, batch_id, item)
        return True
    condition = runner.results_layout(root).raw_path(
        item.experiment, runner.batch_name(batch_id), item.condition
    )
    selection = select_attempt(condition, item.run_index)
    if selection.skip:
        return True
    selection.path.mkdir(parents=True)
    (selection.path / "metadata.json").write_text(
        json.dumps(
            {
                "experiment": item.experiment,
                "git_sha": SOURCE_SHA,
                "git_dirty": False,
                "git_tags": [],
                "throttled": "0x0",
                "evidence_class": "final",
                "thesis_evidence": True,
            }
        )
    )
    if item.experiment == "e-val-1":
        (selection.path / "percentiles.json").write_text(
            json.dumps({"total_count": 1, "p99_ns": 50_000_000})
        )
    runner.write_status(selection.path, item, "passed")
    return True


def use_results_volume(patch: pytest.MonkeyPatch, base: Path) -> tuple[Path, Path, Path]:
    root, volume = base / "checkout", base / "results volume"
    layout = runner.ResultsLayout.resolve(root, volume, mount_check=lambda _: True)
    patch.setattr(runner, "results_layout", lambda _: layout)
    patch.setattr(runner, "run_item", passed_item)
    patch.setattr(runner, "summarise", lambda *_: None)
    return root, volume, layout.manifest_path("canonical-batches", "rpi5-final")


@pytest.fixture(scope="module")
def executed_final_batch(tmp_path_factory: pytest.TempPathFactory) -> Path:
    base = tmp_path_factory.mktemp("final-batch")
    (base / "checkout/eval").mkdir(parents=True)
    (base / "checkout/SOURCE_STATE.json").write_text(
        json.dumps({"git_sha": SOURCE_SHA, "git_dirty": False, "git_tags": []})
    )
    (base / "results volume").mkdir()
    with pytest.MonkeyPatch.context() as patch, contextlib.redirect_stdout(io.StringIO()):
        root, _, _ = use_results_volume(patch, base)
        patch.setattr(
            sys, "argv", ["canonical_runner.py", "--root", str(root), "--execute", "--batch-id", "final"]
        )
        assert runner.main() == 0
    return base


@pytest.fixture
def final_batch(
    executed_final_batch: Path, tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> tuple[Path, Path, Path]:
    """A copy of one full schedule executed with every item stubbed to pass."""
    shutil.copytree(executed_final_batch, tmp_path, dirs_exist_ok=True)
    return use_results_volume(monkeypatch, tmp_path)


def _set_json(path: Path, **changes: object) -> None:
    path.write_text(json.dumps({**json.loads(path.read_text()), **changes}))


def _leaf(volume: Path, experiment: str) -> Path:
    return next((volume / "raw" / experiment / "rpi5-final").rglob("run-01-attempt-01"))


def _exceed_retry_cap(volume: Path) -> None:
    leaf = _leaf(volume, "e-perf-3")
    leaf.rename(leaf.with_name("run-01-attempt-03"))
    for number in (1, 2):
        failed = leaf.with_name(f"run-01-attempt-{number:02d}")
        failed.mkdir()
        (failed / "canonical-status.json").write_text(
            json.dumps(
                {"status": "failed", "failure_class": "infrastructure", "reasons": ["harness-error"]}
            )
        )


def test_execute_records_the_batch_source_and_matrix(final_batch: tuple[Path, Path, Path]) -> None:
    _, _, ledger = final_batch

    batch = json.loads((ledger / "batch.json").read_text())

    assert batch == {
        "schema_version": 1,
        "batch_id": "final",
        "host": "rpi5",
        "source_git_sha": SOURCE_SHA,
        "source_dirty": False,
        "canonical_matrix_sha256": hashlib.sha256(
            runner.CANONICAL_MATRIX_PATH.read_bytes()
        ).hexdigest(),
        "seed": 1729,
        "experiments": sorted(runner.parse_experiments("all")),
        "repetitions": None,
        "thesis_evidence": True,
        "started_at": batch["started_at"],
    }


def test_resume_refuses_another_source_or_matrix(
    final_batch: tuple[Path, Path, Path],
    monkeypatch: pytest.MonkeyPatch,
    capsys: pytest.CaptureFixture[str],
) -> None:
    root, _, ledger = final_batch
    resume = ("--root", str(root), "--execute", "--batch-id", "final")

    assert run_main(monkeypatch, capsys, *resume)[0] == 0
    (root / "SOURCE_STATE.json").write_text(
        json.dumps({"git_sha": "b" * 40, "git_dirty": False, "git_tags": []})
    )
    new_source = run_main(monkeypatch, capsys, *resume)
    (root / "SOURCE_STATE.json").write_text(
        json.dumps({"git_sha": SOURCE_SHA, "git_dirty": False, "git_tags": []})
    )
    _set_json(ledger / "batch.json", canonical_matrix_sha256="c" * 64)
    new_matrix = run_main(monkeypatch, capsys, *resume)

    for code, _, err in (new_source, new_matrix):
        assert code == 2
        assert "one commit and one matrix" in err


def test_final_batch_refuses_a_dirty_source_at_start_and_on_resume(
    final_batch: tuple[Path, Path, Path],
    monkeypatch: pytest.MonkeyPatch,
    capsys: pytest.CaptureFixture[str],
) -> None:
    root, _, ledger = final_batch
    (root / "SOURCE_STATE.json").write_text(
        json.dumps({"git_sha": SOURCE_SHA, "git_dirty": True, "git_tags": []})
    )
    execute = ("--root", str(root), "--execute")

    fresh = run_main(monkeypatch, capsys, *execute, "--batch-id", "fresh")
    resumed = run_main(monkeypatch, capsys, *execute, "--batch-id", "final")
    monkeypatch.setenv(runner.DIAGNOSTIC_REPETITIONS_ENV, "unset")
    diagnostic = run_main(
        monkeypatch, capsys, *execute, "--batch-id", "pilot", "--repetitions", "2",
        "--experiments", "e-val-1",
    )

    for code, _, err in (fresh, resumed):
        assert code == 2
        assert "needs a clean source tree" in err
    assert not (ledger.parent / "rpi5-fresh/batch.json").exists()
    assert diagnostic[0] == 0, diagnostic[2]


def test_execute_refuses_to_resume_an_approved_batch(
    final_batch: tuple[Path, Path, Path],
    monkeypatch: pytest.MonkeyPatch,
    capsys: pytest.CaptureFixture[str],
) -> None:
    root, volume, ledger = final_batch
    assert run_main(monkeypatch, capsys, "--root", str(root), "--approve", "--batch-id", "final")[0] == 0
    sealed = {path: path.read_bytes() for path in volume.rglob("*") if path.is_file()}

    code, _, err = run_main(monkeypatch, capsys, "--root", str(root), "--execute", "--batch-id", "final")

    assert code == 2
    assert "is approved" in err
    assert {path: path.read_bytes() for path in volume.rglob("*") if path.is_file()} == sealed


def test_approve_leaves_finder_metadata_out_of_the_manifest(
    final_batch: tuple[Path, Path, Path],
    monkeypatch: pytest.MonkeyPatch,
    capsys: pytest.CaptureFixture[str],
) -> None:
    root, volume, ledger = final_batch
    leaf = _leaf(volume, "e-perf-1")
    (leaf / ".DS_Store").write_bytes(b"finder")
    (leaf / "._metadata.json").write_bytes(b"appledouble")

    code, _, err = run_main(monkeypatch, capsys, "--root", str(root), "--approve", "--batch-id", "final")

    assert code == 0, err
    listed = (ledger / "raw.sha256").read_text()
    assert "metadata.json" in listed
    assert ".DS_Store" not in listed
    assert "._metadata.json" not in listed


def test_approve_writes_a_verifiable_manifest_and_the_final_batch_entry(
    final_batch: tuple[Path, Path, Path],
    monkeypatch: pytest.MonkeyPatch,
    capsys: pytest.CaptureFixture[str],
) -> None:
    root, volume, ledger = final_batch
    approve = ("--root", str(root), "--approve", "--batch-id", "final")

    code, out, err = run_main(monkeypatch, capsys, *approve)
    first = (root / "eval/final-batches.json").read_text()
    assert run_main(monkeypatch, capsys, *approve)[0] == 0

    assert code == 0, err
    assert "approved rpi5-final" in out
    assert (root / "eval/final-batches.json").read_text() == first
    listed = [line.split("  ", 1)[1] for line in (ledger / "raw.sha256").read_text().splitlines()]
    assert listed == sorted(
        path.relative_to(volume).as_posix()
        for path in volume.rglob("*")
        if path.is_file() and path.name != "raw.sha256"
    )
    assert any(path.startswith("manifests/aliases/e-perf-2/rpi5-final/") for path in listed)
    checker = ["sha256sum"] if shutil.which("sha256sum") else ["shasum", "-a", "256"]
    checked = subprocess.run(
        [*checker, "-c", "manifests/canonical-batches/rpi5-final/raw.sha256"],
        cwd=volume,
        capture_output=True,
        text=True,
        check=False,
    )
    assert checked.returncode == 0, checked.stdout + checked.stderr
    entry = json.loads(first)["batches"]["rpi5"]
    assert json.loads(first)["schema_version"] == 1
    assert entry == {
        "batch_id": "final",
        "wafer_git_sha": SOURCE_SHA,
        "canonical_matrix_sha256": hashlib.sha256(
            runner.CANONICAL_MATRIX_PATH.read_bytes()
        ).hexdigest(),
        "raw_manifest_sha256": hashlib.sha256((ledger / "raw.sha256").read_bytes()).hexdigest(),
        "approved_at": entry["approved_at"],
        "other_final_batches": [],
    }


def test_approve_discloses_other_final_batches_of_the_host(
    final_batch: tuple[Path, Path, Path],
    monkeypatch: pytest.MonkeyPatch,
    capsys: pytest.CaptureFixture[str],
) -> None:
    root, _, ledger = final_batch
    for name, thesis in (("rpi5-earlier", True), ("rpi5-pilot", False), ("jetson-other", True)):
        other = ledger.parent / name
        other.mkdir()
        (other / "batch.json").write_text(json.dumps({"thesis_evidence": thesis}))

    code, out, err = run_main(monkeypatch, capsys, "--root", str(root), "--approve", "--batch-id", "final")

    assert code == 0, err
    entry = json.loads((root / "eval/final-batches.json").read_text())["batches"]["rpi5"]
    assert entry["other_final_batches"] == ["earlier"]
    assert "earlier" in out


@pytest.mark.parametrize(
    ("mutate", "message"),
    [
        (
            lambda root, volume, ledger: _set_json(
                _leaf(volume, "e-perf-1") / "canonical-status.json", status="failed"
            ),
            "runs are not done",
        ),
        (
            lambda root, volume, ledger: _set_json(
                _leaf(volume, "e-perf-3") / "metadata.json", git_dirty=True
            ),
            "git_dirty=True",
        ),
        (
            lambda root, volume, ledger: _set_json(
                _leaf(volume, "e-swap-4") / "metadata.json", git_sha="b" * 40
            ),
            f"git_sha={'b' * 40}",
        ),
        (
            lambda root, volume, ledger: _set_json(ledger / "e-val-1-gate.json", passed=False),
            "e-val-1-gate.json does not report a pass",
        ),
        (
            lambda root, volume, ledger: (root / "eval/final-batches.json").write_text(
                json.dumps({"schema_version": 1, "batches": {"rpi5": {"batch_id": "earlier"}}})
            ),
            "already approves rpi5 batch earlier",
        ),
        (
            lambda root, volume, ledger: _exceed_retry_cap(volume),
            "used more attempts than its retry cap allows",
        ),
    ],
    ids=[
        "incomplete",
        "dirty-leaf",
        "leaf-sha",
        "e-val-1-gate",
        "other-approved",
        "over-retry-cap",
    ],
)
def test_approve_refuses_a_batch_that_is_not_final_evidence(
    final_batch: tuple[Path, Path, Path],
    monkeypatch: pytest.MonkeyPatch,
    capsys: pytest.CaptureFixture[str],
    mutate: object,
    message: str,
) -> None:
    root, volume, ledger = final_batch
    mutate(root, volume, ledger)
    final_batches = root / "eval/final-batches.json"
    before = final_batches.read_bytes() if final_batches.is_file() else None

    code, _, err = run_main(monkeypatch, capsys, "--root", str(root), "--approve", "--batch-id", "final")

    assert code == 1
    assert message in err
    assert not (ledger / "raw.sha256").exists()
    assert (final_batches.read_bytes() if final_batches.is_file() else None) == before


def test_approve_refuses_diagnostic_candidate_and_dirty_batches(
    final_batch: tuple[Path, Path, Path],
    monkeypatch: pytest.MonkeyPatch,
    capsys: pytest.CaptureFixture[str],
) -> None:
    root, _, ledger = final_batch
    monkeypatch.setenv(runner.DIAGNOSTIC_REPETITIONS_ENV, "unset")
    execute = ("--root", str(root), "--execute")
    assert run_main(
        monkeypatch, capsys, *execute, "--batch-id", "pilot", "--repetitions", "2",
        "--experiments", "e-val-1",
    )[0] == 0
    assert run_main(
        monkeypatch, capsys, *execute, "--batch-id", "candidate",
        "--experiments", "e-perf-payload-refinement",
    )[0] == 0
    pilot = json.loads((ledger.parent / "rpi5-pilot/batch.json").read_text())
    assert (pilot["repetitions"], pilot["thesis_evidence"]) == (2, False)
    _set_json(ledger / "batch.json", source_dirty=True)

    refusals = {
        batch_id: run_main(monkeypatch, capsys, "--root", str(root), "--approve", "--batch-id", batch_id)
        for batch_id in ("pilot", "candidate", "final")
    }

    assert all(code == 1 for code, _, _ in refusals.values())
    assert "diagnostic batch" in refusals["pilot"][2]
    assert "only final batches under canonical-batches" in refusals["candidate"][2]
    assert "dirty source tree" in refusals["final"][2]
    assert not (root / "eval/final-batches.json").exists()


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
