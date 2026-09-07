#!/usr/bin/env python3

from __future__ import annotations

import csv
import hashlib
import json
import os
import subprocess
import sys
from pathlib import Path

import pytest

ROOT = Path(__file__).resolve().parents[3]
RUNNER = ROOT / "eval/scripts/characterize-rpi5-host.sh"
PHASES = [
    "idle",
    "sut-core-load-1",
    "sut-core-load-2",
    "sut-core-load-3",
    "cpu-memory",
    "usb-write",
    "usb-read",
    "cpu-memory-usb",
]


def sample(phase: str, index: int = 0) -> dict[str, object]:
    return {
        "timestamp_utc": f"2026-09-07T00:00:{index:02d}Z",
        "monotonic_ns": index * 1_000_000_000,
        "phase": phase,
        "phase_elapsed_seconds": index,
        "boot_id": "boot-a",
        "temperature_millicelsius": 60_000,
        "cpu_frequency_hz": 2_400_000_000,
        "throttled": "0x0",
        "pmic_internal_rail_proxy_watts": 5.25,
        "memory_available_bytes": 2_000_000_000,
        "memory_psi_some_avg10": 0.0,
        "usb_read_bytes_per_second": 0.0,
        "usb_write_bytes_per_second": 0.0,
    }


def fixture() -> dict[str, object]:
    return {
        "boot_id": "boot-a",
        "initial_throttled": "0x0",
        "phases": [
            {
                "name": phase,
                "samples": [sample(phase, index)],
                "kernel_io_errors": [],
                "usb": {
                    "bytes": 1_048_576 if phase in {"usb-write", "usb-read", "cpu-memory-usb"} else 0,
                    "elapsed_seconds": 1.0,
                    "expected_sha256": "a" * 64 if phase in {"usb-write", "usb-read", "cpu-memory-usb"} else None,
                    "observed_sha256": "a" * 64 if phase in {"usb-write", "usb-read", "cpu-memory-usb"} else None,
                },
            }
            for index, phase in enumerate(PHASES)
        ],
    }


def run_fixture(tmp_path: Path, value: dict[str, object]) -> tuple[subprocess.CompletedProcess[str], Path]:
    source = tmp_path / "fixture.json"
    source.write_text(json.dumps(value))
    output = tmp_path / "host characterization"
    completed = subprocess.run(
        [
            str(RUNNER),
            "--output-dir",
            str(output),
            "--session-id",
            "fixture-session",
            "--expected-boot-id",
            "boot-a",
            "--fixture-json",
            str(source),
        ],
        cwd=ROOT,
        env={**os.environ, "WAFER_SOURCE_STATE": str(tmp_path / "missing-source-state.json")},
        capture_output=True,
        text=True,
        check=False,
    )
    return completed, output


def test_complete_ladder_writes_bounded_receipt_and_declared_outputs(tmp_path: Path) -> None:
    completed, output = run_fixture(tmp_path, fixture())

    assert completed.returncode == 0, completed.stderr
    assert sorted(path.name for path in output.iterdir()) == [
        "host-load-ladder.json",
        "host-telemetry.csv",
        "kernel-io.log",
        "usb-integrity.json",
    ]
    receipt = json.loads((output / "host-load-ladder.json").read_text())
    assert receipt["schema_version"] == 1
    assert receipt["experiment_id"] == "e-host-thermal-storage"
    assert receipt["session_id"] == "fixture-session"
    assert receipt["sample_unit"] == "clean-boot host characterization session"
    assert receipt["evidence_class"] == "diagnostic"
    assert receipt["thesis_evidence"] is False
    assert receipt["n30_admitted"] is False
    assert receipt["phase_order"] == PHASES
    assert receipt["phase_durations_seconds"] == {
        "idle": 120,
        "all_other_phases": 300,
    }
    assert receipt["sample_interval_seconds"] == 1.0
    assert receipt["clean_boot_max_age_seconds"] == 600
    assert [phase["name"] for phase in receipt["phases"]] == PHASES
    assert all(phase["status"] == "passed" for phase in receipt["phases"])
    assert receipt["execution_mode"] == "fixture-synthetic"
    assert receipt["admission_eligible"] is False
    assert receipt["evaluated_diagnostic_gate"] is True
    assert receipt["evaluated_final_gate"] is True
    assert receipt["diagnostic_admission"] is False
    assert receipt["final_admission"] is False
    assert receipt["status"] == "synthetic-pass"
    assert receipt["stop_reason"] is None
    assert receipt["power_boundary"]["measurement"] == "rpi5-pmic-internal-rail-proxy"
    assert receipt["power_boundary"]["is_total_input_power"] is False
    assert receipt["source_state"]["provisional"] is True

    with (output / "host-telemetry.csv").open(newline="") as stream:
        rows = list(csv.DictReader(stream))
    assert [row["phase"] for row in rows] == PHASES
    assert set(rows[0]) == {
        "timestamp_utc",
        "monotonic_ns",
        "phase",
        "phase_elapsed_seconds",
        "boot_id",
        "temperature_millicelsius",
        "cpu_frequency_hz",
        "throttled",
        "pmic_internal_rail_proxy_watts",
        "memory_available_bytes",
        "memory_psi_some_avg10",
        "usb_read_bytes_per_second",
        "usb_write_bytes_per_second",
    }
    usb = json.loads((output / "usb-integrity.json").read_text())
    assert usb["checksum_mismatch_count"] == 0
    assert [item["phase"] for item in usb["checks"]] == [
        "usb-write",
        "usb-read",
        "cpu-memory-usb",
    ]
    assert (output / "kernel-io.log").read_text() == ""


@pytest.mark.parametrize(
    ("mutation", "reason"),
    [
        (lambda value: value["phases"][2]["samples"][0].update(temperature_millicelsius=75_000), "temperature-at-or-above-75c"),
        (lambda value: value["phases"][2]["samples"][0].update(throttled="0x80000"), "nonzero-throttling"),
        (lambda value: value["phases"][2]["samples"][0].update(boot_id="boot-b"), "boot-id-changed"),
        (lambda value: value["phases"][2].update(kernel_io_errors=["blk_update_request: I/O error"]), "kernel-io-error"),
        (lambda value: value["phases"][5]["usb"].update(observed_sha256="b" * 64), "usb-checksum-mismatch"),
    ],
)
def test_first_failure_stops_ladder_and_blocks_admission(
    tmp_path: Path, mutation, reason: str
) -> None:
    value = fixture()
    mutation(value)

    completed, output = run_fixture(tmp_path, value)

    assert completed.returncode == 1
    receipt = json.loads((output / "host-load-ladder.json").read_text())
    assert receipt["status"] == "failed"
    assert receipt["stop_reason"] == reason
    assert receipt["diagnostic_admission"] is False
    assert receipt["final_admission"] is False
    failed_index = next(
        index for index, phase in enumerate(receipt["phases"]) if phase["status"] == "failed"
    )
    assert all(phase["status"] == "not-run" for phase in receipt["phases"][failed_index + 1 :])


def test_combined_failure_preserves_diagnostic_gate_but_blocks_final_admission(
    tmp_path: Path,
) -> None:
    value = fixture()
    value["phases"][-1]["samples"][0]["temperature_millicelsius"] = 75_000

    completed, output = run_fixture(tmp_path, value)

    assert completed.returncode == 1
    receipt = json.loads((output / "host-load-ladder.json").read_text())
    assert receipt["evaluated_diagnostic_gate"] is True
    assert receipt["evaluated_final_gate"] is False
    assert receipt["diagnostic_admission"] is False
    assert receipt["final_admission"] is False
    assert receipt["stop_reason"] == "temperature-at-or-above-75c"


def test_clean_boot_identity_and_zero_throttle_are_required(tmp_path: Path) -> None:
    value = fixture()
    value["boot_id"] = "boot-b"

    completed, output = run_fixture(tmp_path, value)

    assert completed.returncode == 1
    receipt = json.loads((output / "host-load-ladder.json").read_text())
    assert receipt["status"] == "failed"
    assert receipt["stop_reason"] == "clean-boot-id-mismatch"
    assert all(phase["status"] == "not-run" for phase in receipt["phases"])

    second_output = tmp_path / "initial-throttle"
    value = fixture()
    value["initial_throttled"] = "0x1"
    fixture_path = tmp_path / "throttle.json"
    fixture_path.write_text(json.dumps(value))
    completed = subprocess.run(
        [
            str(RUNNER),
            "--output-dir",
            str(second_output),
            "--session-id",
            "fixture-throttle",
            "--expected-boot-id",
            "boot-a",
            "--fixture-json",
            str(fixture_path),
        ],
        cwd=ROOT,
        capture_output=True,
        text=True,
        check=False,
    )
    assert completed.returncode == 1
    receipt = json.loads((second_output / "host-load-ladder.json").read_text())
    assert receipt["stop_reason"] == "clean-boot-throttling-history"

    third_output = tmp_path / "stale-boot"
    value = fixture()
    value["boot_age_seconds"] = 601
    fixture_path = tmp_path / "stale-boot.json"
    fixture_path.write_text(json.dumps(value))
    completed = subprocess.run(
        [
            str(RUNNER),
            "--output-dir",
            str(third_output),
            "--session-id",
            "fixture-stale-boot",
            "--expected-boot-id",
            "boot-a",
            "--fixture-json",
            str(fixture_path),
        ],
        cwd=ROOT,
        capture_output=True,
        text=True,
        check=False,
    )
    assert completed.returncode == 1
    receipt = json.loads((third_output / "host-load-ladder.json").read_text())
    assert receipt["stop_reason"] == "clean-boot-age-exceeded"


def test_rejects_unbounded_or_non_monotonic_telemetry(tmp_path: Path) -> None:
    value = fixture()
    value["phases"][0]["samples"] = [sample("idle", index) for index in range(123)]
    source = tmp_path / "unbounded.json"
    source.write_text(json.dumps(value))
    completed = subprocess.run(
        [
            str(RUNNER),
            "--output-dir",
            str(tmp_path / "unbounded-output"),
            "--session-id",
            "fixture-unbounded",
            "--expected-boot-id",
            "boot-a",
            "--fixture-json",
            str(source),
        ],
        cwd=ROOT,
        capture_output=True,
        text=True,
        check=False,
    )
    assert completed.returncode == 2
    assert "maximum telemetry rows" in completed.stderr

    value = fixture()
    value["phases"][0]["samples"] = [sample("idle", 1), sample("idle", 0)]
    source = tmp_path / "non-monotonic.json"
    source.write_text(json.dumps(value))
    completed = subprocess.run(
        [
            str(RUNNER),
            "--output-dir",
            str(tmp_path / "non-monotonic-output"),
            "--session-id",
            "fixture-non-monotonic",
            "--expected-boot-id",
            "boot-a",
            "--fixture-json",
            str(source),
        ],
        cwd=ROOT,
        capture_output=True,
        text=True,
        check=False,
    )
    assert completed.returncode == 2
    assert "strictly increasing monotonic timestamps" in completed.stderr


def test_usb_workers_hash_expected_zero_bytes_and_detect_corruption(tmp_path: Path) -> None:
    corpus = tmp_path / "corpus.bin"
    write = subprocess.run(
        [
            sys.executable,
            str(ROOT / "eval/scripts/lib/host_characterization.py"),
            "worker",
            "write",
            "0",
            f"{corpus}:4096",
        ],
        cwd=ROOT,
        capture_output=True,
        text=True,
        check=False,
    )
    assert write.returncode == 0, write.stderr
    write_result = json.loads(write.stdout)
    assert write_result["expected_sha256"] == hashlib.sha256(bytes(4096)).hexdigest()
    assert write_result["observed_sha256"] == write_result["expected_sha256"]

    corpus.write_bytes(b"changed")
    read = subprocess.run(
        [
            sys.executable,
            str(ROOT / "eval/scripts/lib/host_characterization.py"),
            "worker",
            "read",
            "0",
            str(corpus),
        ],
        cwd=ROOT,
        capture_output=True,
        text=True,
        check=False,
    )
    assert read.returncode == 0, read.stderr
    read_result = json.loads(read.stdout)
    assert read_result["expected_sha256"] == hashlib.sha256(bytes(len(b"changed"))).hexdigest()
    assert read_result["observed_sha256"] == hashlib.sha256(b"changed").hexdigest()
    assert read_result["observed_sha256"] != read_result["expected_sha256"]


def test_matrix_phase_order_and_outputs_match_runner_contract() -> None:
    matrix = json.loads((ROOT / "eval/canonical-matrix.json").read_text())
    experiment = matrix["enhanced_candidate"]["experiments"]["e-host-thermal-storage"]
    assert experiment["conditions"] == PHASES
    assert experiment["required_outputs"] == [
        "host-load-ladder.json",
        "host-telemetry.csv",
        "kernel-io.log",
        "usb-integrity.json",
    ]
    assert experiment["sample_unit"] == "clean-boot host characterization session"
    assert experiment["nested_units"] == [
        "load phase within session",
        "one-second telemetry interval within phase",
    ]


def test_output_is_append_only_and_fixture_requires_exact_phase_order(tmp_path: Path) -> None:
    completed, output = run_fixture(tmp_path, fixture())
    assert completed.returncode == 0

    completed, _ = run_fixture(tmp_path, fixture())
    assert completed.returncode == 2
    assert "already exists" in completed.stderr

    malformed = fixture()
    malformed["phases"][0], malformed["phases"][1] = (
        malformed["phases"][1],
        malformed["phases"][0],
    )
    source = tmp_path / "malformed.json"
    source.write_text(json.dumps(malformed))
    malformed_output = tmp_path / "malformed-output"
    completed = subprocess.run(
        [
            str(RUNNER),
            "--output-dir",
            str(malformed_output),
            "--session-id",
            "fixture-malformed",
            "--expected-boot-id",
            "boot-a",
            "--fixture-json",
            str(source),
        ],
        cwd=ROOT,
        capture_output=True,
        text=True,
        check=False,
    )
    assert completed.returncode == 2
    assert "exact phase order" in completed.stderr
    assert not malformed_output.exists()
