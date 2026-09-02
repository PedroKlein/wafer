#!/usr/bin/env python3

import hashlib
import json
import subprocess
import sys
import tempfile
from pathlib import Path

ROOT = Path(__file__).resolve().parents[3]
VERIFIER = ROOT / "eval/scripts/verify-result-contract.py"


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
    required = matrix["experiments"][experiment]["required_outputs"]
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
            "matrix_sha256": hashlib.sha256(
                (ROOT / "eval/canonical-matrix.json").read_bytes()
            ).hexdigest(),
            "memory_retention_fix_commit": matrix["focused_pilot"]["decisions"]["memory_retention"]["fix_commit"],
            "ekuiper_operator_concurrency": 1,
        },
    }
    if system == "ekuiper":
        metadata["exit_codes"] = {"ekuiper": 0}
        (result / "runtime-provenance.json").unlink()
    (result / "metadata.json").write_text(json.dumps(metadata))
    if "sequence.csv" in required:
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
                    "offered_messages": 1000,
                    "received_messages": 1000,
                    "lost_messages": 0,
                    "gap_messages": 0,
                    "duplicates": 0,
                },
                "branch_b": {"artifact_dir": "branch-b"},
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
        ("e-iso-7", "control", "wafer", "branch-isolation.json", lambda value: value["branches"]["branch_a"].update(gap_messages=1), "branch A is not lossless"),
        ("e-perf-9", "small-cold", "wafer", "startup.json", lambda value: value.update(processed_messages=2), "exactly one processed message"),
        ("e-swap-1", "steady", "wafer", "hotswap-analysis.json", lambda value: value.update(sample_count=49), "must contain 50 events"),
        ("e-swap-5", "process-trap-rollback", "wafer", "rollback.json", lambda value: value.update(rolled_back=49), "50 successful rollbacks"),
        ("e-perf-10", "ekuiper/rate-01000", "ekuiper", "ekuiper-audit.json", lambda value: value["rule"]["options"].update(concurrency=3), "frozen operator concurrency 1"),
        ("e-swap-3", "wafer-hotswap", "wafer", "sequence.csv", None, "sequence.csv is not lossless"),
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


def test_rate_sweep_contract_rejects_missing_resource_field() -> None:
    with tempfile.TemporaryDirectory() as tmp:
        root = Path(tmp)
        result = root / "e-perf-10" / "rpi5-test" / "mqtt-loopback" / "rate-01000" / "run-01"
        result.mkdir(parents=True)
        required = json.loads((ROOT / "eval/canonical-matrix.json").read_text())["experiments"]["e-perf-10"]["required_outputs"]
        for name in {"config.toml", "stdout.log", *required, "pi-telemetry.csv", "pmic-rails.csv", "power-boundary.json"}:
            (result / name).write_text("fixture\n")
        (result / "measurement-window.json").write_text('{"started_ns":100,"finished_ns":200}\n')
        (result / "metadata.json").write_text(
            json.dumps(
                {
                    "experiment": "e-perf-10",
                    "system": "mqtt-loopback",
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
                    "exit_codes": {"publisher": 0, "subscriber": 0},
                }
            )
        )
        sweep = {
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
        (result / "rate-sweep.json").write_text(json.dumps(sweep))
        assert run(result).returncode == 0

        sweep["resources"].pop("cpu_percent")
        (result / "rate-sweep.json").write_text(json.dumps(sweep))
        completed = run(result)
    assert completed.returncode == 1
    assert "resources missing field: cpu_percent" in completed.stdout


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


if __name__ == "__main__":
    test_canonical_result_accepts_complete_leaf()
    test_canonical_ekuiper_result_does_not_require_wasmtime_provenance()
    test_canonical_result_rejects_dirty_untagged_and_missing_output()
    print("canonical result verifier tests: PASS")
