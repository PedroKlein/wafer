#!/usr/bin/env python3

import hashlib
import importlib.util
import json
import subprocess
import sys
import tempfile
from pathlib import Path

ROOT = Path(__file__).resolve().parents[3]
VERIFIER = ROOT / "eval/scripts/verify-result-contract.py"
SPEC = importlib.util.spec_from_file_location("verify_result_contract", VERIFIER)
assert SPEC is not None and SPEC.loader is not None
CONTRACT = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(CONTRACT)


def run(path: Path) -> subprocess.CompletedProcess[str]:
    return subprocess.run(
        [sys.executable, str(VERIFIER), "--canonical", str(path)],
        cwd=ROOT,
        capture_output=True,
        text=True,
        check=False,
    )


def make_result(root: Path) -> Path:
    result = root / "e-perf-4" / "rpi5-2026-08-30T00-00-00Z" / "120b" / "run-01"
    result.mkdir(parents=True)
    for name in (
        "config.toml",
        "stdout.log",
        "runtime-provenance.json",
        "latency.hdr",
        "throughput.csv",
        "sequence.csv",
        "pi-telemetry.csv",
        "pmic-rails.csv",
        "power-boundary.json",
    ):
        (result / name).write_text("fixture\n")
    (result / "measurement-window.json").write_text(
        '{"started_ns":100,"finished_ns":200}\n'
    )
    metadata = {
        "experiment": "e-perf-4",
        "host_tag": "rpi5",
        "hardware_model": "Raspberry Pi 5 Model B Rev 1.0",
        "arch": "aarch64",
        "isolated_cpus": "1-3",
        "cpu_governors": ["performance"],
        "throttled": "0x0",
        "git_sha": "1" * 40,
        "git_dirty": False,
        "git_tags": ["rpi5-eval-v1"],
        "wasmtime_version": "43.0.0",
        "wafer_runtime_sha256": "2" * 64,
        "wafer_plugin_hashes": {"transform": "3" * 64},
        "exit_codes": {"wafer_runtime": 0},
    }
    (result / "metadata.json").write_text(json.dumps(metadata))
    return result


def run_focused(path: Path, matrix: Path | None = None) -> subprocess.CompletedProcess[str]:
    command = [sys.executable, str(VERIFIER), "--canonical", "--focused"]
    if matrix is not None:
        command.extend(["--matrix", str(matrix)])
    command.append(str(path))
    return subprocess.run(command, cwd=ROOT, capture_output=True, text=True, check=False)


def make_focused_result(root: Path, experiment: str, condition: str, system: str = "wafer") -> Path:
    result = root / experiment / "rpi5-focused-test" / Path(condition) / "run-01-attempt-01"
    result.mkdir(parents=True)
    matrix = json.loads((ROOT / "eval/canonical-matrix.json").read_text())
    required = matrix["focused_pilot"]["experiments"][experiment]["required_outputs"]
    for name in {
        "config.toml",
        "stdout.log",
        "runtime-provenance.json",
        "pi-telemetry.csv",
        "pmic-rails.csv",
        "power-boundary.json",
        *required,
    }:
        (result / name).write_text("fixture\n")
    (result / "measurement-window.json").write_text('{"started_ns":100,"finished_ns":200}\n')
    metadata = {
        "experiment": experiment,
        "condition": condition,
        "system": system,
        "thesis_evidence": False,
        "host_tag": "rpi5",
        "hardware_model": "Raspberry Pi 5 Model B Rev 1.0",
        "arch": "aarch64",
        "isolated_cpus": "1-3",
        "cpu_governors": ["performance"],
        "throttled": "0x0",
        "git_sha": "1" * 40,
        "git_dirty": False,
        "git_tags": ["rpi5-eval-v1"],
        "wasmtime_version": "43.0.0",
        "wafer_runtime_sha256": "2" * 64,
        "wafer_plugin_hashes": {"filter": "3" * 64},
        "exit_codes": {"wafer_runtime": 0},
        "focused_pilot": {
            "id": matrix["focused_pilot"]["id"],
            "matrix_sha256": json.loads(
                (ROOT / "eval/focused-pilot-freeze.json").read_text()
            )["canonical_matrix_sha256"],
            "memory_retention_fix_commit": matrix["focused_pilot"]["decisions"]["memory_retention"]["fix_commit"],
            "ekuiper_operator_concurrency": 1,
        },
    }
    if system == "ekuiper":
        metadata["exit_codes"] = {"ekuiper": 0}
        (result / "runtime-provenance.json").unlink()
    (result / "metadata.json").write_text(json.dumps(metadata))
    if "sequence.csv" in required:
        if experiment == "e-swap-3":
            (result / "sequence.csv").write_text(
                "event_type,seq_start,seq_end,count\n"
            )
            (result / "subscriber-metadata.json").write_text(json.dumps({
                "exit_reason": "total-messages",
                "total_messages": 120_000,
                "total_recorded": 120_000,
                "sequence": {
                    "total_received": 120_000,
                    "total_gaps": 0,
                    "total_duplicates": 0,
                    "gap_ranges": [],
                    "duplicate_seqs": [],
                },
            }))
        else:
            (result / "sequence.csv").write_text(
                "total_expected,total_received,gap_ranges,gap_msgs,duplicates_count\n"
                "1000,1000,0,0,0\n"
            )
    if experiment == "e-perf-10":
        sweep = {
            "schema_version": 1,
            "experiment": experiment,
            "system": system,
            "thesis_evidence": False,
            "measurement_boundary": "publisher run window to subscriber receive timestamp",
            "units": {"rate": "messages/second", "latency": "nanoseconds", "rss": "bytes"},
            "offered_rate_msg_s": 1000,
            "actual_offered_rate_msg_s": 1000.0,
            "achieved_rate_msg_s": 1000.0,
            "measurement_duration_ns": 60_000_000_000,
            "messages": {"offered": 60_000, "received": 60_000, "lost": 0, "duplicates": 0},
            "loss_percent": 0.0,
            "latency_ns": {"p50": 1, "p95": 2, "p99": 3},
            "resources": {"scope": "sut", "cpu_percent": 1.0, "max_rss_bytes": 1},
            "throttled": False,
            "profile": {"path": "profile.toml", "sha256": "2" * 64, "payload_template_sha256": "3" * 64},
            "process_audit": {"path": "process-audit.json", "sha256": "4" * 64},
            "traces": {
                "published": {"path": "published.csv", "sha256": "5" * 64, "samples": 60_000},
                "received": {"path": "received.csv", "sha256": "6" * 64, "samples": 60_000},
            },
        }
        (result / "rate-sweep.json").write_text(json.dumps(sweep))
        (result / "subscriber-metadata.json").write_text(
            json.dumps({"sequence_end_exclusive": 60_000, "ignored_sequence_count": 0})
        )
        if system == "ekuiper":
            (result / "ekuiper-audit.json").write_text(
                json.dumps({"rule": {"options": {"concurrency": 1}}})
            )
    if experiment == "e-backpressure":
        (result / "backpressure.json").write_text(json.dumps({
            "classification": "saturated-and-drained",
            "threshold_crossed": True,
            "recovered": True,
            "peak_occupancy": 1.0,
            "occupancy_threshold": 0.8,
            "rates_msg_s": {"offered": 1000.0, "accepted": 150.0, "processed": 150.0, "drained": 140.0},
            "sequence": {"lossless": True},
            "memory": {"within_limit": True},
        }))
    if experiment == "e-iso-4":
        (result / "containment.json").write_text(json.dumps({
            "contained": True,
            "traps_total": 2,
            "nodes": [{"node_id": "attack", "traps_total": "2", "recovery_count": "2"}],
        }))
    if experiment == "e-iso-7":
        (result / "branch-isolation.json").write_text(json.dumps({
            "condition": condition,
            "measurement_boundary": {"kind": "branch_sink_post_warmup"},
            "branches": {
                "branch_a": {
                    "artifact_dir": "branch-a",
                    "source_node": "source_a",
                    "target_messages": 1000,
                    "target_shortfall_messages": 0,
                    "sequence_scope": "post_warmup",
                    "offered_messages": 1000,
                    "received_messages": 1000,
                    "lost_messages": 0,
                    "gap_messages": 0,
                    "duplicates": 0,
                    "measurement_window": {
                        "started_ns": 1_000_000_000,
                        "finished_ns": 61_000_000_000,
                    },
                    "throughput": {"total_messages": 1000},
                    "latency_ns": {"sample_count": 1000},
                },
                "branch_b": {
                    "artifact_dir": "branch-b",
                    "source_node": "source_b",
                    "target_messages": 1000,
                    "target_shortfall_messages": 1000,
                    "sequence_scope": "post_warmup",
                    "offered_messages": 0,
                    "received_messages": 0,
                    "lost_messages": 0,
                    "gap_messages": 0,
                    "duplicates": 0,
                },
            },
        }))
    if experiment == "e-perf-9":
        (result / "startup.json").write_text(json.dumps({
            "schema_version": 1,
            "clock": "monotonic",
            "cache_state": condition.rsplit("-", 1)[1],
            "cache_preparation": {
                "action": "drop-linux-page-cache" if condition.endswith("-cold") else "none",
                "completed_before_timing": True,
            },
            "compiled_component_cache": {"mode": "disabled", "hit": False, "artifact": None, "identity": None},
            "plugin_sha256": {"filter": "3" * 64},
            "processed_messages": 1,
            "phases_ns": {
                "process_config": 1,
                "component_load_compile": 1,
                "instantiation": 1,
                "pipeline_setup": 1,
                "first_process": 1,
            },
            "total_wall_duration_ns": 5,
            "harness_overhead_tolerance_ns": 5_000_000,
        }))
    if experiment in {"e-swap-1", "e-swap-2", "e-swap-4", "e-swap-6"}:
        event = {
            "compile_ns": 1,
            "instantiate_ns": 1,
            "signal_ns": 1,
            "ack_ns": 1,
            "convergence_ns": 1,
            "http_total_ns": 5,
            "sink_observed_output_gap_ns": 1,
        }
        (result / "hotswap-analysis.json").write_text(json.dumps({
            "duration_unit": "ns", "sample_count": 50, "events": [event] * 50,
        }))
    if experiment == "e-swap-5":
        (result / "rollback.json").write_text(json.dumps({
            "attempts": 50, "rolled_back": 50, "all_rolled_back": True,
        }))
    return result


def test_focused_semantic_invariants_reject_malformed_artifacts() -> None:
    cases = [
        ("e-backpressure", "saturated-slow-consumer", "wafer", "backpressure.json", lambda value: value.update(classification="not-saturated"), "backpressure classification"),
        ("e-iso-4", "infinite-loop", "wafer", "containment.json", lambda value: value["nodes"][0].update(recovery_count="1"), "epoch recovery count"),
        ("e-iso-7", "control", "wafer", "branch-isolation.json", lambda value: value["branches"]["branch_a"].update(target_shortfall_messages=1, offered_messages=999, received_messages=999, throughput={"total_messages": 999}, latency_ns={"sample_count": 999}), "branch A is not lossless"),
        ("e-iso-7", "control", "wafer", "branch-isolation.json", lambda value: value["branches"]["branch_a"].update(gap_messages=1), "branch A is not lossless"),
        ("e-iso-7", "control", "wafer", "branch-isolation.json", lambda value: value["branches"]["branch_b"].update(source_node="source_a"), "independent source populations"),
        ("e-iso-7", "control", "wafer", "branch-isolation.json", lambda value: value["branches"]["branch_a"]["latency_ns"].update(sample_count=999), "counts do not share the measurement boundary"),
        ("e-perf-9", "small-cold", "wafer", "startup.json", lambda value: value.update(processed_messages=2), "exactly one processed message"),
        ("e-swap-1", "steady", "wafer", "hotswap-analysis.json", lambda value: value.update(sample_count=49), "must contain 50 events"),
        ("e-swap-5", "process-trap-rollback", "wafer", "rollback.json", lambda value: value.update(rolled_back=49), "50 successful rollbacks"),
        ("e-perf-10", "ekuiper/rate-01000", "ekuiper", "ekuiper-audit.json", lambda value: value["rule"]["options"].update(concurrency=3), "frozen operator concurrency 1"),
        ("e-perf-10", "wafer/rate-01000", "wafer", "subscriber-metadata.json", lambda value: value.update(sequence_end_exclusive=None), "subscriber sequence boundary"),
        (
            "e-swap-3",
            "wafer-hotswap",
            "wafer",
            "subscriber-metadata.json",
            lambda value: value["sequence"].update(total_gaps=1),
            "sequence.csv events differ from subscriber metadata",
        ),
    ]
    for experiment, condition, system, filename, mutate, expected in cases:
        with tempfile.TemporaryDirectory() as tmp:
            result = make_focused_result(Path(tmp), experiment, condition, system)
            assert run_focused(result).returncode == 0, (experiment, run_focused(result).stdout)
            path = result / filename
            if mutate is None:
                path.write_text(
                    "total_expected,total_received,gap_ranges,gap_msgs,duplicates_count\n"
                    "1000,999,1,1,0\n"
                )
            else:
                value = json.loads(path.read_text())
                mutate(value)
                path.write_text(json.dumps(value))
            completed = run_focused(result)
        assert completed.returncode == 1, (experiment, completed.stdout, completed.stderr)
        assert expected in completed.stdout, (experiment, completed.stdout)


def test_focused_verifier_rejects_loss_in_loadgen_sequence_schema() -> None:
    with tempfile.TemporaryDirectory() as tmp:
        result = make_focused_result(
            Path(tmp), "e-swap-3", "wafer-hotswap", "wafer"
        )
        metadata_path = result / "subscriber-metadata.json"
        metadata = json.loads(metadata_path.read_text())
        metadata["sequence"]["total_gaps"] = 1
        metadata["sequence"]["gap_ranges"] = [[42, 42]]
        metadata_path.write_text(json.dumps(metadata))
        (result / "sequence.csv").write_text(
            "event_type,seq_start,seq_end,count\n"
            "gap,42,42,1\n"
        )
        completed = run_focused(result)
    assert completed.returncode == 1
    assert "sequence.csv is not lossless" in completed.stdout


def test_focused_verifier_rejects_incomplete_loadgen_sequence_capture() -> None:
    with tempfile.TemporaryDirectory() as tmp:
        result = make_focused_result(
            Path(tmp), "e-swap-3", "wafer-hotswap", "wafer"
        )
        metadata_path = result / "subscriber-metadata.json"
        metadata = json.loads(metadata_path.read_text())
        metadata["exit_reason"] = "signal"
        metadata_path.write_text(json.dumps(metadata))
        completed = run_focused(result)
    assert completed.returncode == 1
    assert "subscriber sequence metadata is inconsistent" in completed.stdout


def test_focused_verifier_rejects_unresolved_memory_decision() -> None:
    with tempfile.TemporaryDirectory() as tmp:
        root = Path(tmp)
        result = make_focused_result(root, "e-iso-4", "infinite-loop")
        matrix = json.loads((ROOT / "eval/canonical-matrix.json").read_text())
        matrix["focused_pilot"]["decisions"]["memory_retention"]["status"] = "unresolved"
        matrix_path = root / "matrix.json"
        matrix_path.write_text(json.dumps(matrix))
        completed = run_focused(result, matrix_path)
    assert completed.returncode == 1
    assert "memory-retention status is not fixed" in completed.stdout


def test_canonical_result_accepts_complete_leaf() -> None:
    with tempfile.TemporaryDirectory() as tmp:
        result = make_result(Path(tmp))
        completed = run(result)
    assert completed.returncode == 0, completed.stdout + completed.stderr


def test_canonical_ekuiper_result_does_not_require_wasmtime_provenance() -> None:
    with tempfile.TemporaryDirectory() as tmp:
        root = Path(tmp)
        result = root / "e-perf-1" / "rpi5-2026-08-30T00-00-00Z" / "ekuiper" / "run-01"
        result.mkdir(parents=True)
        for name in (
            "config.toml",
            "stdout.log",
            "latency.hdr",
            "throughput.csv",
            "sequence.csv",
            "pi-telemetry.csv",
            "pmic-rails.csv",
            "power-boundary.json",
        ):
            (result / name).write_text("fixture\n")
        (result / "measurement-window.json").write_text(
            '{"started_ns":100,"finished_ns":200}\n'
        )
        metadata = {
            "experiment": "e-perf-1",
            "system": "ekuiper",
            "host_tag": "rpi5",
            "hardware_model": "Raspberry Pi 5 Model B Rev 1.0",
            "arch": "aarch64",
            "isolated_cpus": "1-3",
            "cpu_governors": ["performance"],
            "throttled": "0x0",
            "git_sha": "1" * 40,
            "git_dirty": False,
            "git_tags": ["rpi5-eval-v1"],
            "ekuiper_version": "2.1.0",
            "exit_codes": {"ekuiper": 0},
        }
        (result / "metadata.json").write_text(json.dumps(metadata))
        completed = run(result)
    assert completed.returncode == 0, completed.stdout + completed.stderr


def test_historical_rate_sweep_contract_rejects_missing_resource_field() -> None:
    result = {
        "schema_version": 1,
        "experiment": "e-perf-10",
        "system": "mqtt-loopback",
        "thesis_evidence": False,
        "measurement_boundary": "publisher run window to subscriber receive timestamp",
        "units": {"rate": "messages/second", "latency": "nanoseconds", "rss": "bytes"},
        "offered_rate_msg_s": 1000,
        "actual_offered_rate_msg_s": 1000.0,
        "achieved_rate_msg_s": 999.0,
        "measurement_duration_ns": 60_000_000_000,
        "messages": {"offered": 60_000, "received": 60_000, "lost": 0, "duplicates": 0},
        "loss_percent": 0.0,
        "latency_ns": {"p50": 1, "p95": 2, "p99": 3},
        "resources": {"scope": "no-sut", "cpu_percent": 0.0, "max_rss_bytes": 0},
        "throttled": False,
        "profile": {
            "path": "profile.toml",
            "sha256": "2" * 64,
            "payload_template_sha256": "3" * 64,
        },
        "process_audit": {"path": "process-audit.json", "sha256": "4" * 64},
        "traces": {"published": {}, "received": {}},
    }
    with tempfile.TemporaryDirectory() as tmp:
        path = Path(tmp) / "rate-sweep.json"
        path.write_text(json.dumps(result))
        assert CONTRACT.check_rate_sweep_result(path) == []
        result["resources"].pop("cpu_percent")
        path.write_text(json.dumps(result))
        violations = CONTRACT.check_rate_sweep_result(path)
    assert "resources missing field: cpu_percent" in " ".join(violations)

def test_canonical_result_rejects_dirty_untagged_and_missing_output() -> None:
    mutations = {
        "dirty source": lambda result, metadata: metadata.update(git_dirty=True),
        "tagged source": lambda result, metadata: metadata.update(git_tags=[]),
        "required canonical artefact": lambda result, metadata: (
            result / "latency.hdr"
        ).unlink(),
        "Pi telemetry artefact": lambda result, metadata: (
            result / "measurement-window.json"
        ).unlink(),
        "empty or reversed": lambda result, metadata: (
            result / "measurement-window.json"
        ).write_text('{"started_ns":200,"finished_ns":100}\n'),
    }
    for expected, mutate in mutations.items():
        with tempfile.TemporaryDirectory() as tmp:
            result = make_result(Path(tmp))
            metadata_path = result / "metadata.json"
            metadata = json.loads(metadata_path.read_text())
            mutate(result, metadata)
            metadata_path.write_text(json.dumps(metadata))
            completed = run(result)
        assert completed.returncode == 1, (expected, completed.stdout, completed.stderr)
        assert expected in completed.stdout, (expected, completed.stdout)


def test_final_matrix_missing_new_artifacts_has_experiment_diagnostics() -> None:
    matrix = json.loads((ROOT / "eval/canonical-matrix.json").read_text())
    with tempfile.TemporaryDirectory() as tmp:
        leaf = Path(tmp)
        for name in ("config.toml", "metadata.json", "stdout.log"):
            (leaf / name).write_text("{}\n")
        capacity, _ = CONTRACT.check_leaf(
            leaf, "e-perf-10", canonical=True, canonical_matrix=matrix
        )
        swap, _ = CONTRACT.check_leaf(
            leaf, "e-swap-3", canonical=True, canonical_matrix=matrix
        )
    assert "missing required canonical artefact for e-perf-10: capacity-run.json" in capacity
    assert "missing required canonical artefact for e-swap-3: throughput-buckets.json" in swap
    assert "missing required canonical artefact for e-swap-3: disruption-timeline.json" in swap


def test_final_capacity_and_publisher_schemas_reject_counter_drift() -> None:
    publisher = {
        "schema_version": 1,
        "intended": 60_000,
        "rejected": 100,
        "enqueued": 59_900,
        "measurement_duration_ns": 60_000_000_000,
    }
    with tempfile.TemporaryDirectory() as tmp:
        path = Path(tmp) / "publisher-summary.json"
        path.write_text(json.dumps(publisher))
        assert CONTRACT.check_publisher_summary(path) == []
        publisher["enqueued"] += 1
        path.write_text(json.dumps(publisher))
        assert "counters do not reconcile" in " ".join(CONTRACT.check_publisher_summary(path))

        subscriber = {
            "started_at_ns": 1,
            "ended_at_ns": 2,
            "exit_reason": "total-messages",
            "git_sha": "1" * 40,
            "host_tag": "rpi5",
            "sequence_end_exclusive": 60_000,
            "ignored_sequence_count": 0,
            "unexpected_sequence_count": 0,
            "total_recorded": 60_000,
            "total_messages": 60_000,
            "parse_errors": 0,
            "negative_latency_count": 0,
            "latency_p50_ns": 1,
            "latency_p95_ns": 2,
            "latency_p99_ns": 3,
            "sequence": {"total_received": 60_000, "total_gaps": 0, "total_duplicates": 0},
        }
        path = Path(tmp) / "subscriber-metadata.json"
        path.write_text(json.dumps(subscriber))
        assert CONTRACT.check_subscriber_metadata(path) == []
        subscriber["unexpected_sequence_count"] = 1
        path.write_text(json.dumps(subscriber))
        assert "unexpected_sequence_count must be zero" in " ".join(
            CONTRACT.check_subscriber_metadata(path)
        )

        capacity = {
            "schema_version": 1,
            "experiment": "e-perf-10",
            "system": "wafer",
            "thesis_evidence": True,
            "rate_msg_s": 1000,
            "measurement_duration_ns": 60_000_000_000,
            "messages": {
                "intended": 60_000,
                "rejected": 0,
                "enqueued": 60_000,
                "received_events": 60_000,
                "received_unique": 60_000,
                "downstream_lost": 0,
                "total_undelivered": 0,
                "duplicates": 0,
                "unexpected": 0,
            },
            "rates_msg_s": {"intended": 1000.0, "achieved": 1000.0},
            "loss_percent": 0.0,
            "latency_ns": {"p50": 1, "p95": 2, "p99": 3},
            "resources": {"scope": "sut", "cpu_percent": 1.0, "max_rss_bytes": 1},
            "thermal": {"max_temperature_millicelsius": 60_000, "throttled": False},
            "process_audit": {"path": "process-audit.json", "sha256": "1" * 64},
            "config": {"path": "config.toml", "sha256": "2" * 64},
            "controlled_factors": {"qos": 1},
            "traces": False,
        }
        path = Path(tmp) / "capacity-run.json"
        path.write_text(json.dumps(capacity))
        assert CONTRACT.check_capacity_run_result(path) == []
        capacity["messages"]["received_unique"] -= 1
        path.write_text(json.dumps(capacity))
        assert "enqueued counters do not reconcile" in " ".join(
            CONTRACT.check_capacity_run_result(path)
        )


def test_final_event_and_burst_artifact_schemas_fail_closed() -> None:
    buckets = {
        "schema_version": 1,
        "clock": "monotonic",
        "bucket_width_ns": 100_000_000,
        "received_events": 200,
        "buckets": [
            {
                "start_offset_ns": -10_000_000_000 + index * 100_000_000,
                "end_offset_ns": -10_000_000_000 + (index + 1) * 100_000_000,
                "received_unique": 1,
                "received_events": 1,
                "duplicates": 0,
                "rate_msg_s": 10.0,
            }
            for index in range(200)
        ],
    }
    disruption = {
        "schema_version": 1,
        "clock": "monotonic",
        "strategy": "wafer-hotswap",
        "event_ns": 60,
        "action_start_ns": 60,
        "action_end_ns": 61,
    }
    burst = {
        "schema_version": 1,
        "before_rate_msg_s": 1000,
        "burst_rate_msg_s": 2000,
        "after_rate_msg_s": 1000,
        "burst_start_offset_ns": 55_000_000_000,
        "swap_offset_ns": 60_000_000_000,
        "burst_end_offset_ns": 65_000_000_000,
        "successful_swaps": 1,
    }
    with tempfile.TemporaryDirectory() as tmp:
        root = Path(tmp)
        bucket_path = root / "throughput-buckets.json"
        disruption_path = root / "disruption-timeline.json"
        burst_path = root / "burst-timeline.json"
        bucket_path.write_text(json.dumps(buckets))
        disruption_path.write_text(json.dumps(disruption))
        burst_path.write_text(json.dumps(burst))
        assert CONTRACT.check_throughput_buckets(bucket_path, 200) == []
        assert CONTRACT.check_disruption_timeline(disruption_path) == []
        assert CONTRACT.check_burst_timeline(burst_path) == []
        buckets["buckets"][1]["start_offset_ns"] += 1
        bucket_path.write_text(json.dumps(buckets))
        assert "not contiguous" in " ".join(CONTRACT.check_throughput_buckets(bucket_path, 200))
        burst["successful_swaps"] = 2
        burst_path.write_text(json.dumps(burst))
        assert "successful_swaps must be 1" in " ".join(
            CONTRACT.check_burst_timeline(burst_path)
        )


if __name__ == "__main__":
    test_canonical_result_accepts_complete_leaf()
    test_canonical_ekuiper_result_does_not_require_wasmtime_provenance()
    test_canonical_result_rejects_dirty_untagged_and_missing_output()
    print("canonical result verifier tests: PASS")
