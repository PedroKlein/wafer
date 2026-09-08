#!/usr/bin/env python3

import hashlib
import importlib.util
import json
import subprocess
import sys
import tempfile
import tomllib
from pathlib import Path

ROOT = Path(__file__).resolve().parents[3]
VERIFIER = ROOT / "eval/scripts/verify-result-contract.py"
SPEC = importlib.util.spec_from_file_location("verify_result_contract", VERIFIER)
assert SPEC is not None and SPEC.loader is not None
CONTRACT = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(CONTRACT)


def run(path: Path, *, canonical: bool = True) -> subprocess.CompletedProcess[str]:
    command = [sys.executable, str(VERIFIER)]
    if canonical:
        command.append("--canonical")
    command.append(str(path))
    return subprocess.run(
        command,
        cwd=ROOT,
        capture_output=True,
        text=True,
        check=False,
    )


def test_alias_receipt_dereferences_single_raw_source(tmp_path: Path) -> None:
    source = tmp_path / "raw/e-perf-1/rpi5-batch/native/run-01-attempt-01"
    source.mkdir(parents=True)
    for name in ("config.toml", "metadata.json", "stdout.log"):
        (source / name).write_text("{}" if name.endswith(".json") else "fixture\n")
    status = source / "canonical-status.json"
    status.write_text('{"status":"passed"}')
    receipt = tmp_path / "manifests/aliases/e-perf-2/rpi5-batch/native/run-01.json"
    receipt.parent.mkdir(parents=True)
    receipt.write_text(json.dumps({
        "schema_version": 1,
        "experiment": "e-perf-2",
        "condition": "native",
        "run_index": 1,
        "shared_from_experiment": "e-perf-1",
        "source_leaf": "raw/e-perf-1/rpi5-batch/native/run-01-attempt-01",
        "source_status_sha256": hashlib.sha256(status.read_bytes()).hexdigest(),
        "sample_identity": "raw/e-perf-1/rpi5-batch/native/run-01-attempt-01",
        "shared_measurement": True,
    }))

    result = run(receipt, canonical=False)

    assert result.returncode == 0, result.stdout + result.stderr
    assert "OK: 1 leaf run" in result.stdout


def test_interval_fragment_without_composed_output_is_rejected(tmp_path: Path) -> None:
    leaf = tmp_path / "e-perf-1/run-01"
    leaf.mkdir(parents=True)
    for name in ("config.toml", "metadata.json", "stdout.log"):
        (leaf / name).write_text("{}\n")
    (leaf / "interval-latency.json").write_text("{}\n")

    violations, _ = CONTRACT.check_leaf(leaf, "e-perf-1")

    assert any("lacks composed interval-metrics" in item for item in violations)


def test_invalid_composed_interval_is_rejected(tmp_path: Path) -> None:
    leaf = tmp_path / "e-perf-1/run-01"
    leaf.mkdir(parents=True)
    for name in ("config.toml", "metadata.json", "stdout.log"):
        (leaf / name).write_text("{}\n")
    (leaf / "interval-latency.json").write_text("{}\n")
    (leaf / "interval-metrics.json").write_text('{"schema_version":1,"rows":[]}\n')

    violations, _ = CONTRACT.check_leaf(leaf, "e-perf-1")

    assert any("invalid interval-metrics.json" in item for item in violations)


def test_canonical_timed_leaf_requires_bounded_interval_artifacts(tmp_path: Path) -> None:
    leaf = tmp_path / "e-perf-1/run-01"
    leaf.mkdir(parents=True)
    for name in ("config.toml", "metadata.json", "stdout.log", "latency.hdr"):
        (leaf / name).write_text("{}\n")
    matrix = {
        "experiments": {
            "e-perf-1": {
                "measurement_secs": 60,
                "required_outputs": ["latency.hdr"],
            }
        },
        "enhanced_candidate": {
            "instrumentation": {"bounded_interval_metrics": {"bucket_width_ms": 1000}}
        },
    }

    violations, _ = CONTRACT.check_leaf(
        leaf, "e-perf-1", canonical=True, canonical_matrix=matrix
    )

    assert "missing bounded interval artefact for e-perf-1: interval-latency.json" in violations
    assert "missing bounded interval artefact for e-perf-1: interval-metrics.json" in violations


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
    interval_fragment = {
        "schema_version": 1,
        "interval_clock": "monotonic-elapsed",
        "alignment_clock": "unix-epoch",
        "alignment_clock_purpose": "cross-process-alignment-only",
        "measurement_start_unix_epoch_ns": 1_000_000_000,
        "declared_measurement_duration_ns": 1_000_000_000,
        "bucket_width_ns": 1_000_000_000,
        "maximum_rows": 3,
        "row_count": 1,
        "aggregate_latency_count": 1,
        "late_arrivals": 0,
        "rows": [{
            "interval_start_ns": 0,
            "interval_end_ns": 1_000_000_000,
            "interval_start_unix_epoch_ns": 1_000_000_000,
            "interval_end_unix_epoch_ns": 2_000_000_000,
            "latency_count": 1,
            "latency_p50_ns": 1,
            "latency_p95_ns": 1,
            "latency_p99_ns": 1,
            "received_events": 1,
            "throughput_messages": 1,
            "duplicates": 0,
        }],
    }
    (result / "interval-latency.json").write_text(json.dumps(interval_fragment))
    metric = {"status": "unavailable", "reason": "fixture"}
    interval_metrics = {
        **interval_fragment,
        "sample_unit": "interval-within-run",
        "sources": {"latency_throughput": "interval-latency.json"},
        "estimators": {},
        "rows": [{
            **interval_fragment["rows"][0],
            "latency_p50_ns": {"status": "available", "value": 1},
            "latency_p95_ns": {"status": "available", "value": 1},
            "latency_p99_ns": {"status": "available", "value": 1},
            "throughput_messages_per_second": 1.0,
            "cpu_percent": metric,
            "rss_bytes": metric,
            "pmic_internal_rail_proxy_watts": metric,
            "temperature_millicelsius": metric,
            "queue_depth": metric,
        }],
    }
    (result / "interval-metrics.json").write_text(json.dumps(interval_metrics))
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
        "engine_fuel_budgets": {"transform": 10_000_000, "filter": 500_000, "router": 500_000},
        "epoch_deadline": 100,
        "epoch_tick_ms": 10,
        "effective_metering_mode": "fuel-and-epoch",
        "condition": "120b",
        "system": "wafer",
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


def test_canonical_static_density_result_accepts_release_component_sizes() -> None:
    with tempfile.TemporaryDirectory() as tmp:
        result = (
            Path(tmp)
            / "e-density-1"
            / "rpi5-test"
            / "release-components"
            / "run-01-attempt-01"
        )
        result.mkdir(parents=True)
        for name in (
            "config.toml",
            "stdout.log",
            "binary-sizes.csv",
            "pi-telemetry.csv",
            "pmic-rails.csv",
            "power-boundary.json",
        ):
            (result / name).write_text("fixture\n")
        (result / "measurement-window.json").write_text(
            '{"started_ns":100,"finished_ns":200}\n'
        )
        (result / "metadata.json").write_text(
            json.dumps(
                {
                    "experiment": "e-density-1",
                    "condition": "release-components",
                    "system": "static",
                    "static_measurement": True,
                    "host_tag": "rpi5",
                    "hardware_model": "Raspberry Pi 5 Model B Rev 1.0",
                    "arch": "aarch64",
                    "isolated_cpus": "1-3",
                    "cpu_governors": ["performance"],
                    "throttled": "0x0",
                    "git_sha": "1" * 40,
                    "git_dirty": False,
                    "git_tags": ["rpi5-eval-v1"],
                    "exit_codes": {"collector": 0},
                }
            )
        )
        completed = run(result)
    assert completed.returncode == 0, completed.stdout + completed.stderr


def test_canonical_result_accepts_complete_leaf() -> None:
    with tempfile.TemporaryDirectory() as tmp:
        result = make_result(Path(tmp))
        completed = run(result)
    assert completed.returncode == 0, completed.stdout + completed.stderr



def test_final_wafer_result_rejects_metering_provenance_mismatch() -> None:
    with tempfile.TemporaryDirectory() as tmp:
        result = make_result(Path(tmp))
        metadata_path = result / "metadata.json"
        metadata = json.loads(metadata_path.read_text())
        metadata["epoch_deadline"] = None
        metadata["effective_metering_mode"] = "fuel-only"
        metadata_path.write_text(json.dumps(metadata))
        completed = run(result)
    assert completed.returncode == 1
    assert "metering provenance differs" in completed.stdout

def test_e_perf_5_accepts_transform_only_fuel_for_transform_only_pipeline() -> None:
    with tempfile.TemporaryDirectory() as tmp:
        result = make_result(Path(tmp))
        target = Path(tmp) / "e-perf-5" / "rpi5-2026-08-30T00-00-00Z" / "wafer" / "run-01"
        target.parent.mkdir(parents=True)
        result.rename(target)
        result = target
        metadata_path = result / "metadata.json"
        metadata = json.loads(metadata_path.read_text())
        metadata["experiment"] = "e-perf-5"
        metadata["condition"] = "wafer"
        metadata["engine_fuel_budgets"] = {
            "transform": 10_000_000,
            "filter": None,
            "router": None,
        }
        metadata_path.write_text(json.dumps(metadata))
        completed = run(result)
    assert completed.returncode == 0, completed.stdout + completed.stderr


def test_canonical_ekuiper_result_does_not_require_wasmtime_provenance() -> None:
    with tempfile.TemporaryDirectory() as tmp:
        root = Path(tmp)
        source = make_result(root)
        result = root / "e-perf-1" / "rpi5-2026-08-30T00-00-00Z" / "ekuiper" / "run-01"
        result.parent.mkdir(parents=True)
        source.rename(result)
        (result / "runtime-provenance.json").unlink()
        metadata_path = result / "metadata.json"
        metadata = json.loads(metadata_path.read_text())
        metadata.update(
            experiment="e-perf-1",
            condition="ekuiper",
            system="ekuiper",
            ekuiper_version="2.1.0",
            exit_codes={"ekuiper": 0},
        )
        metadata_path.write_text(json.dumps(metadata))
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
        "deadline_misses": 0,
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
            "histogram_lowest_ns": 1_000,
            "histogram_highest_ns": 10_000_000_000,
            "histogram_sig_digits": 3,
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
            "batch_class": "final-capacity",
            "experiment": "e-perf-10",
            "system": "wafer",
            "thesis_evidence": True,
            "rate_msg_s": 1000,
            "run_index": 1,
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
            "latency_hdr": {
                "path": "latency.hdr",
                "sha256": "3" * 64,
                "samples": 60_000,
                "lowest_ns": 1_000,
                "highest_ns": 10_000_000_000,
                "significant_digits": 3,
            },
            "resources": {"scope": "sut", "cpu_percent": 1.0, "max_rss_bytes": 1},
            "thermal": {"max_temperature_millicelsius": 60_000, "throttled": False},
            "process_audit": {"path": "process-audit.json", "sha256": "1" * 64},
            "config": {"path": "config.toml", "sha256": "2" * 64},
            "loadgen_profile": {"path": "loadgen-profile.toml", "sha256": "4" * 64},
            "provenance": {"path": "metadata.json", "sha256": "5" * 64},
            "controlled_factors": {"qos": 1},
            "traces": False,
        }
        path = Path(tmp) / "capacity-run.json"
        path.write_text(json.dumps(capacity))
        assert CONTRACT.check_capacity_run_result(path) == []

        candidate = json.loads(json.dumps(capacity))
        candidate.update(
            batch_class="candidate-capacity-knee",
            experiment="e-perf-capacity-knee",
            thesis_evidence=False,
            evidence_class="candidate-supplementary",
            n30_admitted=False,
            rate_msg_s=9_000,
        )
        candidate["messages"].update(
            intended=540_000,
            rejected=0,
            enqueued=540_000,
            received_events=540_000,
            received_unique=540_000,
            downstream_lost=0,
            total_undelivered=0,
        )
        candidate["rates_msg_s"] = {
            "intended": 9_000.0,
            "achieved": 9_000.0,
            "achieved_ratio": 1.0,
        }
        candidate["latency_hdr"]["samples"] = 540_000
        path.write_text(json.dumps(candidate))
        assert CONTRACT.check_capacity_run_result(
            path,
            expected_experiment="e-perf-capacity-knee",
            expected_batch_class="candidate-capacity-knee",
            expected_thesis_evidence=False,
            repetitions=5,
            measurement_secs=60,
            require_n30_exclusion=True,
            allowed_rates_by_system={"wafer": {9_000}},
            expected_evidence_class="candidate-supplementary",
        ) == []
        candidate["rate_msg_s"] = 4_000
        path.write_text(json.dumps(candidate))
        assert "outside the system grid" in " ".join(
            CONTRACT.check_capacity_run_result(
                path,
                expected_experiment="e-perf-capacity-knee",
                expected_batch_class="candidate-capacity-knee",
                expected_thesis_evidence=False,
                repetitions=5,
                measurement_secs=60,
                require_n30_exclusion=True,
                allowed_rates_by_system={"wafer": {9_000}},
                expected_evidence_class="candidate-supplementary",
            )
        )

        capacity["measurement_duration_ns"] = 30_000_000_000
        path.write_text(json.dumps(capacity))
        assert "measurement duration differs" in " ".join(
            CONTRACT.check_capacity_run_result(path)
        )
        capacity["measurement_duration_ns"] = 60_000_000_000
        capacity["messages"]["received_unique"] -= 1
        path.write_text(json.dumps(capacity))
        assert "enqueued counters do not reconcile" in " ".join(
            CONTRACT.check_capacity_run_result(path)
        )

        publisher["rejected"] = 0
        publisher["enqueued"] = 60_000
        subscriber["unexpected_sequence_count"] = 0
        capacity["messages"]["received_unique"] = 60_000
        capacity["messages"]["ignored_warmup"] = 0
        leaf = Path(tmp)
        (leaf / "publisher-summary.json").write_text(json.dumps(publisher))
        (leaf / "subscriber-metadata.json").write_text(json.dumps(subscriber))
        for field, name in {
            "latency_hdr": "latency.hdr",
            "process_audit": "process-audit.json",
            "config": "config.toml",
            "loadgen_profile": "loadgen-profile.toml",
            "provenance": "metadata.json",
        }.items():
            artifact = leaf / name
            artifact.write_text(field)
            capacity[field]["sha256"] = hashlib.sha256(artifact.read_bytes()).hexdigest()
        (leaf / "capacity-run.json").write_text(json.dumps(capacity))
        assert CONTRACT.check_capacity_artifact_reconciliation(leaf) == []
        (leaf / "config.toml").write_text("tampered")
        assert "config receipt checksum mismatch" in " ".join(
            CONTRACT.check_capacity_artifact_reconciliation(leaf)
        )


def test_candidate_payload_manifest_rejects_hash_and_identity_drift(tmp_path: Path) -> None:
    config = ROOT / "eval/configs/enhanced/e-perf-payload-8kb.toml"
    leaf = tmp_path / "e-perf-payload-refinement/8kb/run-01-attempt-01"
    leaf.mkdir(parents=True)
    copied = leaf / "config.toml"
    copied.write_bytes(config.read_bytes())
    metadata = {
        "experiment": "e-perf-payload-refinement",
        "condition": "8kb",
        "run_index": 1,
    }
    manifest = {
        "schema_version": 1,
        "batch_class": "candidate-payload-refinement",
        "experiment": "e-perf-payload-refinement",
        "evidence_class": "candidate-supplementary",
        "thesis_evidence": False,
        "n30_admitted": False,
        "sample_unit": "independent host run at one payload size",
        "condition": "8kb",
        "run_index": 1,
        "source_kind": "bench-source",
        "source_node": "source",
        "source_pattern": "repeated-byte-0x42",
        "transform_node": "transform",
        "transform_plugin_path": (
            "../../../plugins/pass-through/target/wasm32-wasip2/release/"
            "wafer_pass_through.wasm"
        ),
        "sink_kind": "bench-sink",
        "sink_node": "sink",
        "edges": [
            {"from": "source", "to": "transform"},
            {"from": "transform", "to": "sink"},
        ],
        "payload_bytes": 8192,
        "payload_sha256": hashlib.sha256(b"B" * 8192).hexdigest(),
        "rate_msg_s": 1_000,
        "warmup_messages": 30_000,
        "measurement_messages": 60_000,
        "total_messages": 90_000,
        "config_sha256": hashlib.sha256(copied.read_bytes()).hexdigest(),
        "no_pool_with": ["e-perf-4", "prior diagnostic rehearsals"],
    }
    path = leaf / "payload-manifest.json"
    path.write_text(json.dumps(manifest))

    assert CONTRACT.check_payload_manifest(path, metadata) == []
    manifest["payload_sha256"] = "0" * 64
    path.write_text(json.dumps(manifest))
    assert "payload checksum differs" in " ".join(
        CONTRACT.check_payload_manifest(path, metadata)
    )

    manifest["payload_sha256"] = hashlib.sha256(b"B" * 8192).hexdigest()
    copied.write_text(copied.read_text().replace("wafer_pass_through.wasm", "other.wasm"))
    manifest["transform_plugin_path"] = "other.wasm"
    manifest["config_sha256"] = hashlib.sha256(copied.read_bytes()).hexdigest()
    path.write_text(json.dumps(manifest))
    assert "candidate identity is invalid" in " ".join(
        CONTRACT.check_payload_manifest(path, metadata)
    )


def test_candidate_topology_manifest_rejects_node_or_metering_drift(tmp_path: Path) -> None:
    config = ROOT / "eval/configs/enhanced/e-perf-depth-20.toml"
    leaf = tmp_path / "e-perf-depth-extension/depth-20/run-01-attempt-01"
    leaf.mkdir(parents=True)
    copied = leaf / "config.toml"
    copied.write_bytes(config.read_bytes())
    raw = tomllib.loads(copied.read_text())
    manifest = {
        "schema_version": 1,
        "batch_class": "candidate-depth-extension",
        "experiment": "e-perf-depth-extension",
        "evidence_class": "candidate-supplementary",
        "thesis_evidence": False,
        "n30_admitted": False,
        "sample_unit": "independent host run at one pipeline depth",
        "condition": "depth-20",
        "run_index": 1,
        "depth": 20,
        "node_count": 22,
        "source_count": 1,
        "source_kind": "bench-source",
        "sink_count": 1,
        "sink_kind": "bench-sink",
        "edge_count": 21,
        "transform_count": 20,
        "node_ids": list(raw["nodes"]),
        "edges": raw["edges"],
        "transform_plugin_paths": sorted(
            {node["plugin"] for node in raw["nodes"].values() if node.get("type") == "transform"}
        ),
        "identical_transform_behavior": True,
        "engine_fuel_budgets": {"transform": 10_000_000, "filter": 500_000, "router": 500_000},
        "epoch_deadline": 100,
        "epoch_tick_ms": 10,
        "effective_metering_mode": "fuel-and-epoch",
        "payload_bytes": 128,
        "rate_msg_s": 1_000,
        "warmup_messages": 30_000,
        "measurement_messages": 60_000,
        "config_sha256": hashlib.sha256(copied.read_bytes()).hexdigest(),
        "no_pool_with": ["e-perf-3", "e-perf-6", "e-perf-8", "prior diagnostic rehearsals"],
    }
    assert len(raw["nodes"]) == manifest["node_count"]
    path = leaf / "topology-manifest.json"
    path.write_text(json.dumps(manifest))

    assert CONTRACT.check_topology_manifest(path, metadata={"condition": "depth-20", "run_index": 1}) == []
    manifest["transform_count"] = 19
    path.write_text(json.dumps(manifest))
    assert "does not reconcile with config.toml" in " ".join(
        CONTRACT.check_topology_manifest(path, metadata={"condition": "depth-20", "run_index": 1})
    )

    manifest["transform_count"] = 20
    copied.write_text(copied.read_text().replace("wafer_pass_through.wasm", "other.wasm"))
    manifest["transform_plugin_paths"] = ["other.wasm"]
    manifest["config_sha256"] = hashlib.sha256(copied.read_bytes()).hexdigest()
    path.write_text(json.dumps(manifest))
    assert "does not reconcile with config.toml" in " ".join(
        CONTRACT.check_topology_manifest(path, metadata={"condition": "depth-20", "run_index": 1})
    )


def candidate_swap_leaf(tmp_path: Path, *, rollback: bool) -> tuple[Path, dict, dict]:
    experiment = (
        "e-swap-rollback-sessions" if rollback else "e-swap-independent-sessions"
    )
    condition = "process-trap-rollback" if rollback else "steady"
    leaf = tmp_path / experiment / condition / "run-03-attempt-01"
    leaf.mkdir(parents=True)
    source_leaf = f"raw/{experiment}/rpi5-test/{condition}/run-03-attempt-01"
    metadata = {
        "experiment": experiment,
        "condition": condition,
        "run_index": 3,
        "measurement_source_leaf": source_leaf,
        "system": "wafer",
        "host_tag": "rpi5",
        "hardware_model": "Raspberry Pi 5 Model B Rev 1.0",
        "arch": "aarch64",
        "isolated_cpus": "1-3",
        "cpu_governors": ["performance"],
        "throttled": "0x0",
        "git_sha": "1" * 40,
        "git_dirty": False,
        "git_tags": ["rpi5-eval-v5"],
        "wasmtime_version": "43.0.0",
        "wafer_runtime_sha256": "2" * 64,
        "wafer_plugin_hashes": {"transform": "3" * 64},
        "engine_fuel_budgets": {
            "transform": 10_000_000,
            "filter": 500_000,
            "router": 500_000,
        },
        "epoch_deadline": 100,
        "epoch_tick_ms": 10,
        "effective_metering_mode": "fuel-and-epoch",
        "exit_codes": {"wafer_runtime": 0},
        "evidence_class": "candidate-supplementary",
        "thesis_evidence": False,
        "n30_admitted": False,
        "batch_class": (
            "candidate-rollback-session" if rollback else "candidate-independent-swap"
        ),
    }
    requests = []
    events = []
    transitions = []
    for index in range(50):
        timeline = {
            "compile_ns": 10 + index,
            "instantiate_ns": 20 + index,
            "signal_ns": 30 + index,
        }
        event = {"event_index": index, "event_class": "first-use-aot" if index == 0 else "cached", **timeline}
        if rollback:
            timeline["rollback_ns"] = 40 + index
            event["rollback_ns"] = 40 + index
        else:
            timeline.update(ack_ns=40 + index, convergence_ns=50 + index)
            event.update(
                ack_ns=40 + index,
                convergence_ns=50 + index,
                sink_observed_output_gap_ns=1_000 + index,
            )
            transitions.append({"pause_ns": 1_000 + index})
        request = {
            "event_index": index,
            "plugin": (
                "wafer_pass_through_v2_panics.wasm"
                if rollback
                else "wafer_pass_through_v2.wasm"
                if index % 2 == 0
                else "wafer_pass_through_v1.wasm"
            ),
            "request_duration_ns": 100 + index,
            "request_duration_clock": "monotonic",
            "http_status": 200,
            "body": {"timeline": timeline},
        }
        if rollback:
            request["body"]["status"] = "rolled_back"
        event.update(
            plugin=request["plugin"],
            http_total_ns=100 + index,
            http_total_clock="monotonic",
        )
        requests.append(request)
        events.append(event)
    (leaf / "swap_requests.json").write_text(json.dumps(requests))
    (leaf / "swap_timeline.json").write_text(json.dumps({"transitions": transitions}))
    (leaf / "sequence.csv").write_text(
        "total_expected,total_received,gap_events,gap_msgs,duplicates_count\n"
        "1000,1000,0,0,0\n"
    )
    evidence = {
        "schema_version": 1,
        "batch_class": "candidate-rollback-session" if rollback else "candidate-independent-swap",
        "experiment": experiment,
        "condition": condition,
        "evidence_class": "candidate-supplementary",
        "thesis_evidence": False,
        "n30_admitted": False,
        "sample_unit": "independent host run",
        "nested_unit": "rollback event within run" if rollback else "swap event within run",
        "run_index": 3,
        "event_classes": ["first-use-aot", "cached"],
        "measurement_source_leaf": source_leaf,
        "shared_from": None,
        "duration_unit": "ns",
        "sample_count": 50,
        "sequence": {"expected": 1_000, "received": 1_000, "gaps": 0, "duplicates": 0},
        "no_pool_with": (
            ["e-swap-5", "prior diagnostic rehearsals"]
            if rollback
            else ["e-swap-1", "e-swap-2", "e-swap-6", "prior diagnostic rehearsals"]
        ),
        "events": events,
    }
    if rollback:
        evidence.update(attempts=50, rolled_back=50, all_rolled_back=True)
    (leaf / ("rollback.json" if rollback else "hotswap-analysis.json")).write_text(
        json.dumps(evidence)
    )
    return leaf, metadata, evidence


def complete_candidate_swap_leaf(leaf: Path, metadata: dict) -> None:
    for name in (
        "config.toml",
        "stdout.log",
        "runtime-provenance.json",
        "latency.hdr",
        "throughput.csv",
        "pi-telemetry.csv",
        "pmic-rails.csv",
        "power-boundary.json",
    ):
        (leaf / name).write_text("fixture\n")
    (leaf / "metadata.json").write_text(json.dumps(metadata))
    (leaf / "measurement-window.json").write_text(
        '{"started_ns":100,"finished_ns":200}\n'
    )
    fragment = {
        "schema_version": 1,
        "interval_clock": "monotonic-elapsed",
        "alignment_clock": "unix-epoch",
        "alignment_clock_purpose": "cross-process-alignment-only",
        "measurement_start_unix_epoch_ns": 1_000_000_000,
        "declared_measurement_duration_ns": 1_000_000_000,
        "bucket_width_ns": 1_000_000_000,
        "maximum_rows": 3,
        "row_count": 1,
        "aggregate_latency_count": 1,
        "late_arrivals": 0,
        "rows": [{
            "interval_start_ns": 0,
            "interval_end_ns": 1_000_000_000,
            "interval_start_unix_epoch_ns": 1_000_000_000,
            "interval_end_unix_epoch_ns": 2_000_000_000,
            "latency_count": 1,
            "latency_p50_ns": 1,
            "latency_p95_ns": 1,
            "latency_p99_ns": 1,
            "received_events": 1,
            "throughput_messages": 1,
            "duplicates": 0,
        }],
    }
    (leaf / "interval-latency.json").write_text(json.dumps(fragment))
    metric = {"status": "unavailable", "reason": "fixture"}
    interval_metrics = {
        **{key: value for key, value in fragment.items() if key != "rows"},
        "sample_unit": "interval-within-run",
        "sources": {"latency_throughput": "interval-latency.json"},
        "estimators": {},
        "rows": [{
            **fragment["rows"][0],
            "latency_p50_ns": {"status": "available", "value": 1},
            "latency_p95_ns": {"status": "available", "value": 1},
            "latency_p99_ns": {"status": "available", "value": 1},
            "throughput_messages_per_second": 1.0,
            "cpu_percent": metric,
            "rss_bytes": metric,
            "pmic_internal_rail_proxy_watts": metric,
            "temperature_millicelsius": metric,
            "queue_depth": metric,
        }],
    }
    (leaf / "interval-metrics.json").write_text(json.dumps(interval_metrics))


def test_candidate_swap_full_leaf_contract_accepts_real_required_outputs(
    tmp_path: Path,
) -> None:
    matrix = json.loads((ROOT / "eval/canonical-matrix.json").read_text())
    for rollback in (False, True):
        leaf, metadata, _ = candidate_swap_leaf(tmp_path, rollback=rollback)
        complete_candidate_swap_leaf(leaf, metadata)
        if rollback:
            (leaf / "swap_timeline.json").unlink()
        violations, warnings = CONTRACT.check_leaf(
            leaf,
            metadata["experiment"],
            canonical=True,
            canonical_matrix=matrix,
        )
        assert violations == []
        assert warnings == []


def test_candidate_swap_verifier_rejects_missing_or_mixed_events(tmp_path: Path) -> None:
    leaf, metadata, evidence = candidate_swap_leaf(tmp_path, rollback=False)
    assert CONTRACT.check_candidate_swap_evidence(
        leaf, metadata, "e-swap-independent-sessions"
    ) == []

    evidence["events"][0]["event_class"] = "cached"
    (leaf / "hotswap-analysis.json").write_text(json.dumps(evidence))
    assert "event labels are invalid" in " ".join(
        CONTRACT.check_candidate_swap_evidence(
            leaf, metadata, "e-swap-independent-sessions"
        )
    )

    evidence["events"] = evidence["events"][:-1]
    evidence["sample_count"] = 49
    (leaf / "hotswap-analysis.json").write_text(json.dumps(evidence))
    assert "exactly 50 events" in " ".join(
        CONTRACT.check_candidate_swap_evidence(
            leaf, metadata, "e-swap-independent-sessions"
        )
    )


def test_candidate_rollback_verifier_reconciles_all_events(tmp_path: Path) -> None:
    leaf, metadata, evidence = candidate_swap_leaf(tmp_path, rollback=True)
    assert CONTRACT.check_candidate_swap_evidence(
        leaf, metadata, "e-swap-rollback-sessions"
    ) == []

    evidence["events"][10]["rollback_ns"] += 1
    (leaf / "rollback.json").write_text(json.dumps(evidence))
    assert "does not reconcile" in " ".join(
        CONTRACT.check_candidate_swap_evidence(
            leaf, metadata, "e-swap-rollback-sessions"
        )
    )


def test_final_event_and_burst_artifact_schemas_fail_closed() -> None:
    buckets = {
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
        "received_unique": 200,
        "received_events": 200,
        "duplicates": 0,
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
        "timestamp_clock": "unix-epoch",
        "timestamp_clock_purpose": "cross-process-alignment",
        "scheduling_clock": "monotonic",
        "duration_clock": "monotonic",
        "strategy": "wafer-hotswap",
        "measurement_start_timestamp_ns": 1_000_000_000_000,
        "scheduled_event_timestamp_ns": 1_060_000_000_000,
        "scheduled_event_offset_ns": 60_000_000_000,
        "event_timestamp_ns": 1_060_005_000_000,
        "event_offset_from_measurement_start_ns": 60_005_000_000,
        "alignment_error_ns": 5_000_000,
        "alignment_tolerance_ns": 10_000_000,
        "action_start_timestamp_ns": 1_060_005_000_000,
        "action_end_timestamp_ns": 1_060_005_000_001,
        "action_end_offset_ns": 1,
        "action_start_monotonic_ns": 5_000_000_000,
        "action_end_monotonic_ns": 5_000_000_001,
        "action_duration_ns": 1,
    }
    burst_measurement_start = 1_000_000_000_000
    burst = {
        "schema_version": 1,
        "timestamp_clock": "unix-epoch",
        "timestamp_clock_purpose": "cross-process-alignment",
        "scheduling_clock": "monotonic",
        "measurement_start_ns": burst_measurement_start,
        "burst_start_ns": burst_measurement_start + 55_000_000_000,
        "scheduled_swap_ns": burst_measurement_start + 60_000_000_000,
        "swap_ns": burst_measurement_start + 60_005_000_000,
        "burst_end_ns": burst_measurement_start + 65_000_000_000,
        "measurement_end_ns": burst_measurement_start + 120_000_000_000,
        "source_completion_offset_ns": 120_000_000_000,
        "before_rate_msg_s": 1000,
        "burst_rate_msg_s": 2000,
        "after_rate_msg_s": 1000,
        "burst_start_offset_ns": 55_000_000_000,
        "scheduled_swap_offset_ns": 60_000_000_000,
        "actual_swap_offset_ns": 60_005_000_000,
        "burst_end_offset_ns": 65_000_000_000,
        "swap_alignment_error_ns": 5_000_000,
        "swap_alignment_tolerance_ns": 10_000_000,
        "successful_swaps": 1,
        "phases": {
            "before": {"rate_msg_s": 1000, "start_offset_ns": 0, "end_offset_ns": 55_000_000_000, "intended": 55_000, "emitted": 55_000, "received": 55_000},
            "burst": {"rate_msg_s": 2000, "start_offset_ns": 55_000_000_000, "end_offset_ns": 65_000_000_000, "intended": 20_000, "emitted": 20_000, "received": 20_000},
            "after": {"rate_msg_s": 1000, "start_offset_ns": 65_000_000_000, "end_offset_ns": 120_000_000_000, "intended": 55_000, "emitted": 55_000, "received": 55_000},
        },
        "sequence": {"expected": 130_000, "received": 130_000, "gaps": 0, "duplicates": 0},
        "loss": 0,
        "primary_received_events": 130_000,
        "drain_received_events": 0,
        "drain_first_offset_ns": None,
        "drain_last_offset_ns": None,
        "drain_duration_after_window_ns": 0,
        "max_arrival_offset_ns": 119_999_000_000,
        "drain_right_censored": False,
        "internal_swap_phases_ns": {"compile_ns": 1, "instantiate_ns": 1, "signal_ns": 1, "ack_ns": 1, "convergence_ns": 1},
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
        fine_path = root / "throughput-buckets-10ms.json"
        fine_path.write_text(json.dumps(fine_bucket_fixture(buckets, "e-swap-3")))
        assert CONTRACT.check_fine_event_buckets(
            fine_path, bucket_path, experiment="e-swap-3"
        ) == []
        assert CONTRACT.check_disruption_timeline(disruption_path) == []
        assert CONTRACT.check_burst_timeline(burst_path) == []
        buckets["alignment_error_ns"] = 10_000_001
        bucket_path.write_text(json.dumps(buckets))
        assert "event placement" in " ".join(
            CONTRACT.check_throughput_buckets(bucket_path, 200)
        )
        buckets["alignment_error_ns"] = 5_000_000
        buckets["buckets"][1]["start_offset_ns"] += 1
        bucket_path.write_text(json.dumps(buckets))
        assert "not contiguous" in " ".join(CONTRACT.check_throughput_buckets(bucket_path, 200))
        disruption["action_duration_ns"] = 2
        disruption_path.write_text(json.dumps(disruption))
        assert "clocks are invalid" in " ".join(
            CONTRACT.check_disruption_timeline(disruption_path)
        )
        burst["successful_swaps"] = 2
        burst_path.write_text(json.dumps(burst))
        assert "successful_swaps must be 1" in " ".join(
            CONTRACT.check_burst_timeline(burst_path)
        )
        burst["successful_swaps"] = 1
        burst["phases"]["burst"]["rate_msg_s"] = 1000
        burst_path.write_text(json.dumps(burst))
        assert "burst phase is invalid" in " ".join(
            CONTRACT.check_burst_timeline(burst_path)
        )


def swap4_throughput_fixture() -> dict:
    primary = [
        {
            "start_offset_ns": index * 100_000_000,
            "end_offset_ns": (index + 1) * 100_000_000,
            "received_unique": 0,
            "received_events": 0,
            "duplicates": 0,
            "rate_msg_s": 0,
        }
        for index in range(1_200)
    ]
    primary[0].update(received_unique=129_999, received_events=129_999, rate_msg_s=1_299_990)
    drain = [
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
    return {
        "schema_version": 1,
        "clock": "unix-epoch-source-sink-alignment",
        "source_measurement_start_unix_ns": 1_000_000_000_000,
        "origin_mismatch_events": 0,
        "missing_origin_events": 0,
        "bucket_width_ns": 100_000_000,
        "coverage_start_offset_ns": 0,
        "coverage_end_offset_ns": 120_000_000_000,
        "primary_buckets": primary,
        "primary_received_unique": 129_999,
        "primary_received_events": 129_999,
        "primary_duplicates": 0,
        "primary_last_offset_ns": 99_999_999,
        "drain_coverage_start_offset_ns": 120_000_000_000,
        "drain_coverage_end_offset_ns": 130_000_000_000,
        "drain_buckets": drain,
        "drain_received_unique": 1,
        "drain_received_events": 1,
        "drain_duplicates": 0,
        "drain_first_offset_ns": 120_001_000_000,
        "drain_last_offset_ns": 120_001_000_000,
        "after_drain_unique": 0,
        "after_drain_events": 0,
        "after_drain_duplicates": 0,
        "after_drain_first_offset_ns": None,
        "after_drain_last_offset_ns": None,
        "drain_right_censored": False,
        "max_arrival_offset_ns": 120_001_000_000,
        "received_unique": 130_000,
        "received_events": 130_000,
        "duplicates": 0,
        "phase_received_messages": [55_000, 20_000, 55_000],
    }


def swap4_timeline_fixture() -> dict:
    measurement_start = 1_000_000_000_000
    return {
        "schema_version": 1,
        "timestamp_clock": "unix-epoch",
        "timestamp_clock_purpose": "cross-process-alignment",
        "scheduling_clock": "monotonic",
        "measurement_start_ns": measurement_start,
        "burst_start_ns": measurement_start + 55_000_000_000,
        "scheduled_swap_ns": measurement_start + 60_000_000_000,
        "swap_ns": measurement_start + 60_005_000_000,
        "burst_end_ns": measurement_start + 65_000_000_000,
        "measurement_end_ns": measurement_start + 120_001_000_000,
        "source_completion_offset_ns": 120_001_000_000,
        "before_rate_msg_s": 1000,
        "burst_rate_msg_s": 2000,
        "after_rate_msg_s": 1000,
        "burst_start_offset_ns": 55_000_000_000,
        "scheduled_swap_offset_ns": 60_000_000_000,
        "actual_swap_offset_ns": 60_005_000_000,
        "burst_end_offset_ns": 65_000_000_000,
        "swap_alignment_error_ns": 5_000_000,
        "swap_alignment_tolerance_ns": 10_000_000,
        "successful_swaps": 1,
        "phases": {
            "before": {"rate_msg_s": 1000, "start_offset_ns": 0, "end_offset_ns": 55_000_000_000, "intended": 55_000, "emitted": 55_000, "received": 55_000},
            "burst": {"rate_msg_s": 2000, "start_offset_ns": 55_000_000_000, "end_offset_ns": 65_000_000_000, "intended": 20_000, "emitted": 20_000, "received": 20_000},
            "after": {"rate_msg_s": 1000, "start_offset_ns": 65_000_000_000, "end_offset_ns": 120_000_000_000, "intended": 55_000, "emitted": 55_000, "received": 55_000},
        },
        "sequence": {"expected": 130_000, "received": 130_000, "gaps": 0, "duplicates": 0},
        "loss": 0,
        "primary_received_events": 129_999,
        "drain_received_events": 1,
        "drain_first_offset_ns": 120_001_000_000,
        "drain_last_offset_ns": 120_001_000_000,
        "drain_duration_after_window_ns": 1_000_000,
        "max_arrival_offset_ns": 120_001_000_000,
        "drain_right_censored": False,
        "sink_observed_output_gap_ns": 80_000_000,
        "internal_swap_phases_ns": {"compile_ns": 1, "instantiate_ns": 1, "signal_ns": 1, "ack_ns": 1, "convergence_ns": 1},
    }


def fine_bucket_fixture(canonical: dict, experiment: str) -> dict:
    event_timestamp_ns = (
        canonical["event_timestamp_ns"]
        if experiment == "e-swap-3"
        else canonical["source_measurement_start_unix_ns"] + 60_005_000_000
    )
    scheduled_timestamp_ns = (
        canonical["scheduled_event_timestamp_ns"]
        if experiment == "e-swap-3"
        else canonical["source_measurement_start_unix_ns"] + 60_000_000_000
    )
    fine = []
    for index in range(400):
        start = -2_000_000_000 + index * 10_000_000
        unique = 1 if experiment == "e-swap-4" else 0
        fine.append({
            "start_offset_ns": start,
            "end_offset_ns": start + 10_000_000,
            "received_unique": unique,
            "received_events": unique,
            "duplicates": 0,
            "rate_msg_s": unique * 100,
        })
    if experiment == "e-swap-3":
        for parent_index, canonical_parent in enumerate(canonical["buckets"][80:120]):
            fine[parent_index * 10].update(
                received_unique=canonical_parent["received_unique"],
                received_events=canonical_parent["received_events"],
                rate_msg_s=canonical_parent["received_unique"] * 100,
            )
    parents = []
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


def test_fine_event_contract_rejects_non_nested_and_wrong_t0(tmp_path: Path) -> None:
    canonical = swap4_throughput_fixture()
    fine = fine_bucket_fixture(canonical, "e-swap-4")
    canonical_path = tmp_path / "throughput-buckets.json"
    fine_path = tmp_path / "throughput-buckets-10ms.json"
    canonical_path.write_text(json.dumps(canonical))
    fine_path.write_text(json.dumps(fine))
    assert CONTRACT.check_fine_event_buckets(
        fine_path, canonical_path, experiment="e-swap-4"
    ) == []

    fine["parent_buckets"][0]["received_events"] += 1
    fine_path.write_text(json.dumps(fine))
    assert "reconcile" in " ".join(
        CONTRACT.check_fine_event_buckets(
            fine_path, canonical_path, experiment="e-swap-4"
        )
    )
    fine = fine_bucket_fixture(canonical, "e-swap-4")
    fine["event_timestamp_ns"] += 10_000_001
    fine_path.write_text(json.dumps(fine))
    assert "actual-t0" in " ".join(
        CONTRACT.check_fine_event_buckets(
            fine_path, canonical_path, experiment="e-swap-4"
        )
    )


def test_swap4_drain_contract_is_strict(tmp_path: Path) -> None:
    valid = swap4_throughput_fixture()
    path = tmp_path / "throughput-buckets.json"
    path.write_text(json.dumps(valid))
    assert CONTRACT.check_throughput_buckets(path, 1_200) == []
    mutations = [
        ("clock", "monotonic", "source-origin clock"),
        ("source_measurement_start_unix_ns", None, "source-origin clock"),
        ("drain_first_offset_ns", None, "drain offsets"),
        ("drain_right_censored", True, "right-censored"),
    ]
    for field, value, message in mutations:
        invalid = json.loads(json.dumps(valid))
        invalid[field] = value
        assert message in " ".join(CONTRACT._check_swap4_throughput(invalid))
    invalid = json.loads(json.dumps(valid))
    invalid["drain_buckets"][1]["start_offset_ns"] += 1
    assert "drain buckets" in " ".join(CONTRACT._check_swap4_throughput(invalid))
    invalid = json.loads(json.dumps(valid))
    invalid["primary_buckets"].pop()
    assert "1200 primary" in " ".join(CONTRACT._check_swap4_throughput(invalid))
    invalid = json.loads(json.dumps(valid))
    invalid["max_arrival_offset_ns"] = invalid["primary_last_offset_ns"]
    assert "maximum arrival" in " ".join(CONTRACT._check_swap4_throughput(invalid))
    invalid = json.loads(json.dumps(valid))
    invalid.update(after_drain_unique=1, after_drain_events=1,
                   after_drain_first_offset_ns=130_000_000_000,
                   after_drain_last_offset_ns=130_000_000_000,
                   drain_right_censored=True, received_unique=130_001,
                   received_events=130_001, max_arrival_offset_ns=130_000_000_000)
    assert "right-censored" in " ".join(CONTRACT._check_swap4_throughput(invalid))


def ekuiper_profile_contract_fixture(tmp_path: Path, state: str) -> tuple[Path, dict]:
    leaf = tmp_path / state
    leaf.mkdir(parents=True)
    interval = leaf / "interval-metrics.json"
    interval.write_text('{"aggregate_latency_count":240000}\n')
    metadata = {
        "experiment": "e-compare-ekuiper-profile",
        "system": "ekuiper",
        "condition": f"rate-04000/{state}",
        "run_index": 2,
        "offered_rate_msg_s": 4_000,
        "evidence_class": "diagnostic",
        "thesis_evidence": False,
        "n30_admitted": False,
        "batch_class": "diagnostic-ekuiper-profile",
        "shared_measurement": False,
        "measurement_source_leaf": (
            f"raw/e-compare-ekuiper-profile/rate-04000/{state}/run-02-attempt-01"
        ),
        "git_sha": "a" * 40,
        "git_dirty": False,
    }
    process = {
        "status": "unavailable",
        "reason": "process-profiler-disabled-by-design",
    }
    if state == "profiled":
        profile = leaf / "resource-usage.csv"
        profile.write_text("header\nrow\n")
        process = {
            "status": "available",
            "path": profile.name,
            "sha256": hashlib.sha256(profile.read_bytes()).hexdigest(),
            "row_count": 2,
            "maximum_rows": 62,
        }
    runtime = {
        "schema_version": 1,
        "experiment": "e-compare-ekuiper-profile",
        "evidence_class": "diagnostic",
        "thesis_evidence": False,
        "n30_admitted": False,
        "sample_unit": "independent host run at one rate and profiler state",
        "condition": metadata["condition"],
        "run_index": 2,
        "rate_msg_s": 4_000,
        "profiler_state": state,
        "source_git_sha": "a" * 40,
        "source_dirty": False,
        "measurement_source_leaf": metadata["measurement_source_leaf"],
        "shared_from": None,
        "interval_alignment": {
            "clock": "unix-epoch",
            "measurement_start_ns": 10_000_000_000,
            "measurement_end_ns": 70_000_000_000,
            "row_count": 60,
            "path": interval.name,
            "sha256": hashlib.sha256(interval.read_bytes()).hexdigest(),
        },
        "process_metrics": process,
        "latency_ns": {
            "sample_count": 240_000,
            "p50": 100_000,
            "p95": 200_000,
            "p99": 300_000,
        },
        "gc_runtime_metrics": {
            "status": "unavailable",
            "reason": "ekuiper-2.1.0-has-no-validated-gc-event-interface",
        },
        "claim_boundary": "diagnostic-association-only-not-gc-causality",
        "no_pool_with": ["e-perf-1", "e-perf-10", "prior diagnostic rehearsals"],
    }
    paired = "unprofiled-control" if state == "profiled" else "profiled"
    overhead = {
        "schema_version": 1,
        "experiment": "e-compare-ekuiper-profile",
        "condition": metadata["condition"],
        "run_index": 2,
        "rate_msg_s": 4_000,
        "profiler_state": state,
        "paired_condition": f"rate-04000/{paired}",
        "pair_key": "rate-04000/run-02",
        "profile_collection_enabled": state == "profiled",
        "overhead_role": (
            "sampler-enabled" if state == "profiled" else "unprofiled-control"
        ),
        "overhead_estimator": "paired-run-level-profiled-minus-unprofiled-control",
        "claim_boundary": "diagnostic-association-only-not-gc-causality",
    }
    (leaf / "ekuiper-runtime-summary.json").write_text(json.dumps(runtime))
    (leaf / "profiler-overhead.json").write_text(json.dumps(overhead))
    return leaf, metadata


def test_ekuiper_profile_verifier_enforces_diagnostic_pairing_and_limitations(
    tmp_path: Path,
) -> None:
    for state in ("profiled", "unprofiled-control"):
        leaf, metadata = ekuiper_profile_contract_fixture(tmp_path, state)
        assert CONTRACT.check_ekuiper_profile_artifacts(leaf, metadata) == []

    leaf, metadata = ekuiper_profile_contract_fixture(
        tmp_path / "terminal-partial", "unprofiled-control"
    )
    runtime_path = leaf / "ekuiper-runtime-summary.json"
    runtime = json.loads(runtime_path.read_text())
    runtime["interval_alignment"]["row_count"] = 61
    runtime_path.write_text(json.dumps(runtime))
    assert CONTRACT.check_ekuiper_profile_artifacts(leaf, metadata) == []
    runtime["interval_alignment"]["row_count"] = 62
    runtime_path.write_text(json.dumps(runtime))
    assert CONTRACT.check_ekuiper_profile_artifacts(leaf, metadata) == []
    runtime["interval_alignment"]["row_count"] = 63
    runtime_path.write_text(json.dumps(runtime))
    assert "interval alignment" in " ".join(
        CONTRACT.check_ekuiper_profile_artifacts(leaf, metadata)
    )

    leaf, metadata = ekuiper_profile_contract_fixture(tmp_path / "invalid", "profiled")
    runtime_path = leaf / "ekuiper-runtime-summary.json"
    runtime = json.loads(runtime_path.read_text())
    runtime["claim_boundary"] = "GC caused latency tails"
    runtime_path.write_text(json.dumps(runtime))
    assert "diagnostic identity" in " ".join(
        CONTRACT.check_ekuiper_profile_artifacts(leaf, metadata)
    )

    runtime["claim_boundary"] = "diagnostic-association-only-not-gc-causality"
    runtime["process_metrics"]["row_count"] = 63
    runtime_path.write_text(json.dumps(runtime))
    assert "unbounded" in " ".join(
        CONTRACT.check_ekuiper_profile_artifacts(leaf, metadata)
    )


def test_swap4_cross_artifacts_reconcile_source_primary_and_drain(tmp_path: Path) -> None:
    timeline = swap4_timeline_fixture()
    throughput = swap4_throughput_fixture()
    (tmp_path / "burst-timeline.json").write_text(json.dumps(timeline))
    (tmp_path / "throughput-buckets.json").write_text(json.dumps(throughput))
    (tmp_path / "burst-source-timing.json").write_text(
        json.dumps({"measurement_start_ns": timeline["measurement_start_ns"]})
    )
    (tmp_path / "burst-source-summary.json").write_text(json.dumps({
        "measurement_start_ns": timeline["measurement_start_ns"],
        "source_completion_offset_ns": timeline["source_completion_offset_ns"],
    }))
    (tmp_path / "swap_timeline.json").write_text(
        json.dumps({"transitions": [{"pause_ns": 80_000_000}]})
    )
    (tmp_path / "hotswap-analysis.json").write_text(json.dumps({
        "sample_count": 1,
        "events": [{"sink_observed_output_gap_ns": 80_000_000}],
    }))
    (tmp_path / "swap_requests.json").write_text(json.dumps([{
        "request_started_ns": timeline["swap_ns"],
        "body": {"timeline": timeline["internal_swap_phases_ns"]},
    }]))
    assert CONTRACT.check_burst_timeline(tmp_path / "burst-timeline.json") == []
    assert CONTRACT.check_swap4_reconciliation(tmp_path) == []
    invalid_timeline = json.loads(json.dumps(timeline))
    invalid_timeline["source_completion_offset_ns"] = 130_000_000_000
    (tmp_path / "burst-timeline.json").write_text(json.dumps(invalid_timeline))
    assert "clocks" in " ".join(CONTRACT.check_burst_timeline(tmp_path / "burst-timeline.json"))
    (tmp_path / "burst-timeline.json").write_text(json.dumps(timeline))
    invalid_timeline = json.loads(json.dumps(timeline))
    invalid_timeline["phases"]["after"]["received"] -= 1
    (tmp_path / "burst-timeline.json").write_text(json.dumps(invalid_timeline))
    assert "phase receive" in " ".join(CONTRACT.check_swap4_reconciliation(tmp_path))
    (tmp_path / "burst-timeline.json").write_text(json.dumps(timeline))
    source = json.loads((tmp_path / "burst-source-summary.json").read_text())
    source["source_completion_offset_ns"] += 1
    (tmp_path / "burst-source-summary.json").write_text(json.dumps(source))
    assert "source timing" in " ".join(CONTRACT.check_swap4_reconciliation(tmp_path))

if __name__ == "__main__":
    test_canonical_result_accepts_complete_leaf()
    test_canonical_ekuiper_result_does_not_require_wasmtime_provenance()
    test_canonical_result_rejects_dirty_untagged_and_missing_output()
    print("canonical result verifier tests: PASS")
