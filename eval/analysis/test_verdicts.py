"""Tests for eval/analysis/src/wafer_analysis/verdicts.py."""

from __future__ import annotations

import json
import pathlib

import pandas as pd
import pytest

from wafer_analysis import paths
from wafer_analysis.verdicts import (
    Threshold,
    bound_verdict,
    combined_verdict,
    concordance,
    concordance_table,
    count_verdict,
    declared_concordance,
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
    columns = bound_verdict("gap", threshold(5.0, "<"), bounds, estimate=sum(bounds) / 2)
    assert columns == {
        "gap_verdict": verdict,
        "gap_estimate": sum(bounds) / 2,
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
    columns = bound_verdict("achieved", threshold(0.99, ">="), bounds, estimate=bounds[0])
    assert columns["achieved_verdict"] == verdict
    assert columns["achieved_flips_at"] == flips_at


def test_a_bound_on_the_threshold_respects_the_declared_strictness() -> None:
    def verdict(direction: str, bounds: tuple[float, float]) -> str:
        return bound_verdict("x", threshold(5.0, direction), bounds, estimate=5.0)["x_verdict"]

    assert verdict("<", (4.0, 5.0)) == "INCONCLUSIVE"
    assert verdict("<=", (4.0, 5.0)) == "PASS"
    assert verdict("<", (5.0, 6.0)) == "FAIL"
    assert verdict("<=", (5.0, 6.0)) == "INCONCLUSIVE"


def test_a_criterion_without_a_population_is_pending() -> None:
    assert bound_verdict("dip", threshold(5.0, "<"), None, estimate=None) == {
        "dip_verdict": "PENDING",
        "dip_estimate": None,
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


def test_declared_concordance_reads_the_canonical_matrix() -> None:
    rule = declared_concordance()
    assert (rule.canonical_host, rule.replication_hosts) == ("rpi5", ("jetson", "x86"))
    assert rule.criteria_rule == "one-sided-bound"


@pytest.mark.parametrize(
    ("edit", "message"),
    [
        (lambda matrix: matrix.pop("replication_concordance"), "declares no replication_concordance"),
        (
            lambda matrix: matrix["replication_concordance"]["classes"].reverse(),
            "replication_concordance classes differ",
        ),
    ],
)
def test_declared_concordance_rejects_a_rule_the_analysis_does_not_apply(
    tmp_path: pathlib.Path, edit, message: str
) -> None:
    with pytest.raises(ValueError, match=message):
        declared_concordance(write_matrix(tmp_path, edit))


@pytest.mark.parametrize(
    ("canonical", "replication", "expected"),
    [
        (("PASS", 1.2), ("PASS", 1.5), "same-verdict"),
        (("INCONCLUSIVE", 1.9), ("INCONCLUSIVE", 2.1), "same-verdict"),
        (("PASS", 1.2), ("INCONCLUSIVE", 1.8), "same-direction"),
        (("FAIL", 2.6), ("INCONCLUSIVE", 2.1), "same-direction"),
        (("PASS", 1.2), ("INCONCLUSIVE", 2.0), "same-direction"),
        (("PASS", 1.2), ("FAIL", 2.5), "opposite-direction"),
        (("INCONCLUSIVE", 1.9), ("FAIL", 2.4), "opposite-direction"),
        (("PASS", 1.2), ("PENDING", None), "not-estimable"),
        (("PENDING", None), ("PASS", 1.2), "not-estimable"),
        (("PASS", 1.2), (None, None), "not-estimable"),
        (("PASS", None), ("PASS", 1.2), "not-estimable"),
        (("PASS", 1.2), ("PASS", float("nan")), "not-estimable"),
    ],
)
def test_replication_concordance_classes(
    canonical: tuple[str | None, float | None],
    replication: tuple[str | None, float | None],
    expected: str,
) -> None:
    assert concordance(threshold(2.0, "<="), *canonical, *replication) == expected


def ratio_table(rows: dict[str, tuple[str | None, float | None]]) -> pd.DataFrame:
    return pd.DataFrame(
        [
            {
                "condition": condition,
                "p95_ratio_verdict": verdict,
                "p95_ratio_estimate": estimate,
                "thesis_evidence": True,
            }
            for condition, (verdict, estimate) in rows.items()
        ]
    )


def test_concordance_table_compares_each_replication_host_with_the_canonical_verdict() -> None:
    pi = ratio_table({"wafer": ("PASS", 1.2), "native": (None, None)})
    before = pi.copy()
    jetson = ratio_table({"wafer": ("FAIL", 2.4), "native": (None, None)})

    table = concordance_table({"rpi5": pi, "jetson": jetson}, {"p95_ratio": "e-perf-1-p95-ratio"})

    assert table[["criterion", "condition", "host", "concordance"]].values.tolist() == [
        ["e-perf-1-p95-ratio", "wafer", "jetson", "opposite-direction"],
        ["e-perf-1-p95-ratio", "wafer", "x86", "not-estimable"],
    ]
    assert table["canonical_verdict"].eq("PASS").all()
    assert table["canonical_side"].tolist() == ["meets", "meets"]
    assert table["replication_side"].iloc[0] == "misses"
    assert pd.isna(table["replication_side"].iloc[1])
    assert pd.isna(table["replication_verdict"].iloc[1])
    assert table["thesis_evidence"].tolist() == [True, False]
    assert (table["threshold"].eq(2.0) & table["direction"].eq("<=")).all()
    pd.testing.assert_frame_equal(pi, before)


def test_concordance_table_refuses_a_criterion_without_bounds() -> None:
    with pytest.raises(ValueError, match="e-perf-1-duplicates is not a one-sided-bound criterion"):
        concordance_table({}, {"duplicates": "e-perf-1-duplicates"})


def test_concordance_table_matches_rows_by_the_named_key() -> None:
    rows = ratio_table({"wafer-hotswap": ("PASS", 1.0)}).rename(columns={"condition": "strategy"})
    table = concordance_table(
        {"rpi5": rows, "jetson": rows, "x86": rows}, {"p95_ratio": "e-perf-1-p95-ratio"}, key="strategy"
    )
    assert table[["strategy", "concordance"]].values.tolist() == [
        ["wafer-hotswap", "same-verdict"],
        ["wafer-hotswap", "same-verdict"],
    ]
