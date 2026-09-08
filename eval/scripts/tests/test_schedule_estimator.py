from __future__ import annotations

import importlib.util
import json
import subprocess
import sys
from argparse import Namespace
from pathlib import Path

import pytest

ROOT = Path(__file__).resolve().parents[3]
SCRIPT = ROOT / "eval/scripts/estimate-final-schedule.py"


def load_estimator():
    spec = importlib.util.spec_from_file_location("schedule_estimator", SCRIPT)
    assert spec is not None and spec.loader is not None
    module = importlib.util.module_from_spec(spec)
    sys.modules[spec.name] = module
    spec.loader.exec_module(module)
    return module


def run(*args: str) -> subprocess.CompletedProcess[str]:
    return subprocess.run(
        [sys.executable, str(SCRIPT), *args],
        cwd=ROOT,
        capture_output=True,
        text=True,
        check=False,
    )


def estimate(tmp_path: Path) -> tuple[Path, Path]:
    output = tmp_path / "schedule-estimates.json"
    independent = tmp_path / "schedule-independent.json"
    completed = run(
        "estimate",
        "--output",
        str(output),
        "--independent-output",
        str(independent),
    )
    assert completed.returncode == 0, completed.stderr
    assert "schedule estimates: PASS" in completed.stdout
    return output, independent


def test_estimates_exact_schedule_counts_runtime_files_and_bytes(tmp_path: Path) -> None:
    output, independent = estimate(tmp_path)
    receipt = json.loads(output.read_text())
    independent_receipt = json.loads(independent.read_text())

    assert receipt["provisional"] == (
        receipt["source"]["wafer"]["dirty"]
        or receipt["source"]["tcc_doc"]["dirty"]
    )
    assert receipt["campaign_started"] is False
    assert receipt["source"]["canonical_matrix_sha256"] == (
        "cd4b161a1c5d2a7db631357879ea73722f25fca2d19c954c94d6020f7aed67ed"
    )
    expected = {
        "expanded-n5": {
            "schedule_records": 661,
            "measured_or_static_leaves": 624,
            "shared_aliases": 37,
            "nominal_seconds": 69_980,
            "estimated_file_count": 10_746,
            "estimated_bytes": 17_258_032_804,
        },
        "all-candidate-n30": {
            "schedule_records": 3_936,
            "measured_or_static_leaves": 3_724,
            "shared_aliases": 212,
            "nominal_seconds": 406_380,
            "estimated_file_count": 63_702,
            "estimated_bytes": 85_581_776_354,
        },
        "primary-only-n30": {
            "schedule_records": 2_105,
            "measured_or_static_leaves": 1_893,
            "shared_aliases": 212,
            "nominal_seconds": 163_860,
            "estimated_file_count": 30_813,
            "estimated_bytes": 42_116_112_343,
        },
    }
    for name, values in expected.items():
        observed = receipt["scenarios"][name]
        assert {key: observed[key] for key in values} == values
        assert observed["required_free_bytes_for_2x_margin"] == 2 * observed[
            "estimated_bytes"
        ]
        assert observed["fits_with_2x_margin"] is True
        assert observed["required_free_bytes_for_2x_margin"] <= observed[
            "usb_capacity_bytes"
        ]
        assert {
            key: observed[key]
            for key in (
                "schedule_records",
                "measured_or_static_leaves",
                "shared_aliases",
                "host_sessions",
                "nominal_seconds",
            )
        } == independent_receipt["scenarios"][name]


def test_estimator_counts_alias_receipts_without_duplicate_raw_bytes(
    tmp_path: Path,
) -> None:
    output, _ = estimate(tmp_path)
    scenarios = json.loads(output.read_text())["scenarios"]

    primary = scenarios["primary-only-n30"]
    assert primary["schedule_records"] == (
        primary["measured_or_static_leaves"] + primary["shared_aliases"]
    )
    assert primary["alias_receipt_file_count"] == primary["shared_aliases"]
    assert primary["alias_receipt_bytes"] == primary["shared_aliases"] * 4_096
    assert primary["raw_bytes"] < primary["schedule_records"] * 17_564_330


def test_estimator_includes_interval_host_reserve_and_derived_overhead(
    tmp_path: Path,
) -> None:
    output, _ = estimate(tmp_path)
    receipt = json.loads(output.read_text())
    assumptions = receipt["assumptions"]
    expanded = receipt["scenarios"]["expanded-n5"]

    assert assumptions["interval_bytes_per_row"] == 2_048
    assert assumptions["interrupted_attempt_reserve_percent"] == 25
    assert assumptions["host_usb_corpus_bytes"] == 2 * 1024**3
    assert expanded["interval_file_count"] > 0
    assert expanded["interval_output_bytes"] > 0
    assert expanded["interrupted_attempt_reserve_bytes"] > 0
    assert expanded["derived_report_bytes"] == 768 * 1024**2
    assert expanded["host_sessions"] == 1


def test_verify_rejects_independent_recomputation_drift(tmp_path: Path) -> None:
    output, independent = estimate(tmp_path)
    value = json.loads(independent.read_text())
    value["scenarios"]["expanded-n5"]["schedule_records"] += 1
    independent.write_text(json.dumps(value))

    completed = run(
        "verify",
        "--output",
        str(output),
        "--independent-output",
        str(independent),
    )

    assert completed.returncode == 1
    assert "differ from current source state" in completed.stderr


def test_verify_rejects_source_state_drift(tmp_path: Path) -> None:
    output, independent = estimate(tmp_path)
    value = json.loads(output.read_text())
    value["source"]["wafer"]["status_sha256"] = "0" * 64
    output.write_text(json.dumps(value))

    completed = run(
        "verify",
        "--output",
        str(output),
        "--independent-output",
        str(independent),
    )

    assert completed.returncode == 1
    assert "differ from current source state" in completed.stderr


def test_verify_rejects_expanded_n5_without_two_x_margin(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    estimator = load_estimator()
    monkeypatch.setattr(estimator, "USB_CAPACITY_BYTES", 1)
    matrix = estimator.matrix_document()
    estimates = estimator.runner_estimates(matrix)
    independent = estimator.independent_estimates(matrix)
    output = tmp_path / "schedule-estimates.json"
    independent_output = tmp_path / "schedule-independent.json"
    estimator.write_json(output, estimator.receipt("runner-derived", estimates, ROOT))
    estimator.write_json(
        independent_output,
        estimator.receipt("matrix-independent-recomputation", independent, ROOT),
    )

    with pytest.raises(ValueError, match="expanded N=5 does not fit"):
        estimator.verify(
            Namespace(
                output=output,
                independent_output=independent_output,
                tcc_root=ROOT,
            )
        )
