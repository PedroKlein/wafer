"""Tests for eval/analysis/src/wafer_analysis/verdicts.py."""

from __future__ import annotations

import json
import pathlib

import pytest

from wafer_analysis import paths
from wafer_analysis.verdicts import (
    Threshold,
    bound_verdict,
    combined_verdict,
    count_verdict,
    declared_thresholds,
)

MATRIX = paths._find_repo_root() / "eval/canonical-matrix.json"


def threshold(value: float, direction: str) -> Threshold:
    return Threshold("example", value, "units", direction, "one-sided-bound", 0.95)


def write_matrix(tmp_path: pathlib.Path, edit) -> pathlib.Path:
    matrix = json.loads(MATRIX.read_text())
    edit(matrix)
    path = tmp_path / "canonical-matrix.json"
    path.write_text(json.dumps(matrix))
    return path


def test_declared_thresholds_read_the_canonical_matrix() -> None:
    rules = declared_thresholds()
    gap = rules["e-swap-4-p95-gap"]
    assert (gap.value, gap.unit, gap.direction) == (100_000_000, "ns", "<")
    assert gap.one_sided_confidence == 0.95
    assert gap.interval == 0.9
    assert rules["e-perf-10-competitive-ratio"].rule == "tested-rate-bracket"
    assert rules["e-iso-containment"].rule == "exact-count"


@pytest.mark.parametrize(
    ("bounds", "verdict", "flips_at"),
    [
        ((1.0, 4.0), "PASS", 4.0),
        ((3.0, 9.0), "INCONCLUSIVE", 3.0),
        ((6.0, 9.0), "FAIL", 6.0),
    ],
)
def test_lower_is_better_passes_on_the_upper_bound_and_fails_on_the_lower(
    bounds: tuple[float, float], verdict: str, flips_at: float
) -> None:
    columns = bound_verdict("gap", threshold(5.0, "<"), bounds)
    assert columns == {
        "gap_verdict": verdict,
        "gap_threshold": 5.0,
        "gap_flips_at": flips_at,
        "gap_ci_half_width": (bounds[1] - bounds[0]) / 2,
    }


@pytest.mark.parametrize(
    ("bounds", "verdict", "flips_at"),
    [
        ((0.995, 1.0), "PASS", 0.995),
        ((0.98, 0.995), "INCONCLUSIVE", 0.995),
        ((0.9, 0.95), "FAIL", 0.95),
    ],
)
def test_higher_is_better_passes_on_the_lower_bound_and_fails_on_the_upper(
    bounds: tuple[float, float], verdict: str, flips_at: float
) -> None:
    columns = bound_verdict("achieved", threshold(0.99, ">="), bounds)
    assert columns["achieved_verdict"] == verdict
    assert columns["achieved_flips_at"] == flips_at


def test_a_bound_on_the_threshold_respects_the_declared_strictness() -> None:
    assert bound_verdict("x", threshold(5.0, "<"), (4.0, 5.0))["x_verdict"] == "INCONCLUSIVE"
    assert bound_verdict("x", threshold(5.0, "<="), (4.0, 5.0))["x_verdict"] == "PASS"
    assert bound_verdict("x", threshold(5.0, "<"), (5.0, 6.0))["x_verdict"] == "FAIL"
    assert bound_verdict("x", threshold(5.0, "<="), (5.0, 6.0))["x_verdict"] == "INCONCLUSIVE"


def test_a_criterion_without_a_population_is_pending() -> None:
    assert bound_verdict("dip", threshold(5.0, "<"), None) == {
        "dip_verdict": "PENDING",
        "dip_threshold": 5.0,
        "dip_flips_at": None,
        "dip_ci_half_width": None,
    }


def test_zero_tolerance_counts_are_exact() -> None:
    escapes = declared_thresholds()["e-iso-containment"]
    assert count_verdict(escapes, 0) == "PASS"
    assert count_verdict(escapes, 1) == "FAIL"


def test_combined_verdict_lets_a_failure_outrank_a_missing_or_open_criterion() -> None:
    assert combined_verdict("PASS", "PASS", None) == "PASS"
    assert combined_verdict("PASS", "INCONCLUSIVE") == "INCONCLUSIVE"
    assert combined_verdict("INCONCLUSIVE", "PENDING") == "PENDING"
    assert combined_verdict("PENDING", "FAIL", "PASS") == "FAIL"


@pytest.mark.parametrize(
    ("edit", "message"),
    [
        (lambda matrix: matrix.pop("verdict_rules"), "declares no verdict_rules"),
        (lambda matrix: matrix["verdict_rules"].pop("thresholds"), "declares no verdict_rules"),
        (
            lambda matrix: matrix["verdict_rules"].update(one_sided_confidence=95),
            "one_sided_confidence",
        ),
        (
            lambda matrix: matrix["verdict_rules"]["thresholds"][0].pop("origin"),
            "malformed verdict threshold",
        ),
        (
            lambda matrix: matrix["verdict_rules"]["thresholds"][0].update(direction="=="),
            "malformed verdict threshold",
        ),
        (
            lambda matrix: matrix["verdict_rules"]["thresholds"][0].update(value="5"),
            "malformed verdict threshold",
        ),
        (
            lambda matrix: matrix["verdict_rules"]["thresholds"].append(
                matrix["verdict_rules"]["thresholds"][0]
            ),
            "malformed verdict threshold",
        ),
    ],
)
def test_declared_thresholds_reject_a_missing_or_malformed_table(
    tmp_path: pathlib.Path, edit, message: str
) -> None:
    with pytest.raises(ValueError, match=message):
        declared_thresholds(write_matrix(tmp_path, edit))
