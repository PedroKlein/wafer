#!/usr/bin/env python3

import copy
import json
import subprocess
import sys
import tempfile
from pathlib import Path

ROOT = Path(__file__).resolve().parents[3]
MATRIX = ROOT / "eval/canonical-matrix.json"
DECISION = ROOT / ".plans/rpi5-v5-enhanced-experiment-readiness/architecture-decision.json"
CONTRACT = ROOT / "eval/RESULT-CONTRACT.md"
VALIDATOR = ROOT / "eval/scripts/validate-enhanced-architecture.py"


def run_validator(matrix: Path = MATRIX, decision: Path = DECISION, contract: Path = CONTRACT) -> subprocess.CompletedProcess[str]:
    return subprocess.run(
        [sys.executable, str(VALIDATOR), "--matrix", str(matrix), "--decision", str(decision), "--contract", str(contract)],
        cwd=ROOT,
        capture_output=True,
        text=True,
        check=False,
    )


def test_enhanced_architecture_is_complete_and_candidate_only() -> None:
    result = run_validator()
    assert result.returncode == 0, result.stderr
    assert "enhanced architecture: PASS" in result.stdout
    assert "7 candidate experiments" in result.stdout


def test_exfat_volume_label_fits_format_limit() -> None:
    matrix = json.loads(MATRIX.read_text())
    storage = matrix["enhanced_candidate"]["storage"]
    label = storage["volume_label"]
    assert len(label.encode("utf-16-le")) // 2 <= 11, (
        "exFAT volume labels are limited to 11 UTF-16 code units"
    )
    assert storage["mount_paths"]["macos"] == f"/Volumes/{label}"

    matrix["enhanced_candidate"]["storage"]["volume_label"] = "WAFER_RESULTS"
    with tempfile.TemporaryDirectory() as tmp:
        path = Path(tmp) / "matrix.json"
        path.write_text(json.dumps(matrix))
        result = run_validator(matrix=path)
    assert result.returncode == 1
    assert "exFAT volume label exceeds 11 UTF-16 code units" in result.stderr


def test_enhanced_architecture_rejects_missing_capability_and_final_promotion() -> None:
    matrix = json.loads(MATRIX.read_text())
    mutations = (
        (
            "required capability bounded-interval-metrics",
            lambda value: value["enhanced_candidate"]["capabilities"].remove(
                "bounded-interval-metrics"
            ),
        ),
        (
            "candidate e-perf-capacity-knee thesis_evidence must be false",
            lambda value: value["enhanced_candidate"]["experiments"][
                "e-perf-capacity-knee"
            ].update(thesis_evidence=True),
        ),
        (
            "candidate e-perf-capacity-knee n30_admitted must be false",
            lambda value: value["enhanced_candidate"]["experiments"][
                "e-perf-capacity-knee"
            ].update(n30_admitted=True),
        ),
    )
    for expected, mutate in mutations:
        changed = copy.deepcopy(matrix)
        mutate(changed)
        with tempfile.TemporaryDirectory() as tmp:
            path = Path(tmp) / "matrix.json"
            path.write_text(json.dumps(changed))
            result = run_validator(matrix=path)
        assert result.returncode == 1, expected
        assert expected in result.stderr


def test_enhanced_architecture_rejects_sample_unit_grid_and_output_drift() -> None:
    matrix = json.loads(MATRIX.read_text())
    mutations = (
        (
            "candidate e-perf-depth-extension sample_unit differs",
            lambda value: value["enhanced_candidate"]["experiments"][
                "e-perf-depth-extension"
            ].update(sample_unit="event"),
        ),
        (
            "payload-refinement condition grid differs",
            lambda value: value["enhanced_candidate"]["experiments"][
                "e-perf-payload-refinement"
            ]["payload_bytes"].pop(),
        ),
        (
            "depth-extension condition grid differs",
            lambda value: value["enhanced_candidate"]["experiments"][
                "e-perf-depth-extension"
            ].update(execution_path="MQTT source -> transforms -> MQTT sink"),
        ),
        (
            "eKuiper profile diagnostic contract differs from the frozen contract",
            lambda value: value["enhanced_candidate"]["experiments"][
                "e-compare-ekuiper-profile"
            ]["profiler"].update(maximum_rows_per_run=63),
        ),
        (
            "candidate e-swap-rollback-sessions required_outputs differ",
            lambda value: value["enhanced_candidate"]["experiments"][
                "e-swap-rollback-sessions"
            ]["required_outputs"].remove("rollback.json"),
        ),
    )
    for expected, mutate in mutations:
        changed = copy.deepcopy(matrix)
        mutate(changed)
        with tempfile.TemporaryDirectory() as tmp:
            path = Path(tmp) / "matrix.json"
            path.write_text(json.dumps(changed))
            result = run_validator(matrix=path)
        assert result.returncode == 1, expected
        assert expected in result.stderr


def test_enhanced_architecture_rejects_capacity_knee_execution_or_estimator_drift() -> None:
    mutations = (
        lambda value: value["enhanced_candidate"]["experiments"][
            "e-perf-capacity-knee"
        ]["ordering"].update(cooldown_secs=0),
        lambda value: value["enhanced_candidate"]["experiments"][
            "e-perf-capacity-knee"
        ]["delivery_good"].update(max_loss_percent=2.0),
    )
    for mutate in mutations:
        matrix = json.loads(MATRIX.read_text())
        mutate(matrix)
        with tempfile.TemporaryDirectory() as tmp:
            path = Path(tmp) / "matrix.json"
            path.write_text(json.dumps(matrix))
            result = run_validator(matrix=path)
        assert result.returncode == 1
        assert "capacity-knee execution or estimator contract differs" in result.stderr


def test_enhanced_architecture_rejects_primary_invariant_drift() -> None:
    matrix = json.loads(MATRIX.read_text())
    matrix["experiments"]["e-perf-10"]["capacity_envelope"]["max_loss_percent"] = 2.0
    with tempfile.TemporaryDirectory() as tmp:
        path = Path(tmp) / "matrix.json"
        path.write_text(json.dumps(matrix))
        result = run_validator(matrix=path)
    assert result.returncode == 1
    assert "e-perf-10 delivery-good estimator differs from the frozen primary" in result.stderr


def test_enhanced_architecture_rejects_alias_or_swap_boundary_drift() -> None:
    for expected, mutate in (
        (
            "canonical alias identity differs from the frozen primary",
            lambda value: value["experiments"]["e-swap-2"].update(
                shares_measurements_with=[]
            ),
        ),
        (
            "e-swap-4 primary/drain boundary differs from the frozen primary",
            lambda value: value["experiments"]["e-swap-4"][
                "sink_tail_policy"
            ].update(drain_bucket_count=101),
        ),
    ):
        matrix = json.loads(MATRIX.read_text())
        mutate(matrix)
        with tempfile.TemporaryDirectory() as tmp:
            path = Path(tmp) / "matrix.json"
            path.write_text(json.dumps(matrix))
            result = run_validator(matrix=path)
        assert result.returncode == 1, expected
        assert expected in result.stderr


def test_enhanced_architecture_rejects_missing_claim_qualifier() -> None:
    with tempfile.TemporaryDirectory() as tmp:
        path = Path(tmp) / "RESULT-CONTRACT.md"
        path.write_text(CONTRACT.read_text().replace("not total input power", "not the deferred metric"))
        result = run_validator(contract=path)
    assert result.returncode == 1
    assert "result contract missing required phrase: not total input power" in result.stderr


def test_enhanced_architecture_rejects_trace_or_deferred_claim_drift() -> None:
    for expected, mutate in (
        (
            "e-perf-10 final outputs must remain trace-free",
            lambda value: value["experiments"]["e-perf-10"][
                "required_outputs"
            ].append("published.csv"),
        ),
        (
            "e-perf-5 must remain PENDING",
            lambda value: value["experiments"]["e-perf-5"].update(
                claim_status="complete"
            ),
        ),
        (
            "PMIC measurement boundary must remain internal-rail-proxy",
            lambda value: value["enhanced_candidate"]["deferred_claims"].update(
                pmic_measurement_boundary="total-input-power"
            ),
        ),
    ):
        matrix = json.loads(MATRIX.read_text())
        mutate(matrix)
        with tempfile.TemporaryDirectory() as tmp:
            path = Path(tmp) / "matrix.json"
            path.write_text(json.dumps(matrix))
            result = run_validator(matrix=path)
        assert result.returncode == 1, expected
        assert expected in result.stderr
