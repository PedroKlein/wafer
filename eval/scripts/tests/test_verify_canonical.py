#!/usr/bin/env python3

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
