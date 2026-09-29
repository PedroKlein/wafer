from __future__ import annotations

import numpy as np
import pytest

from wafer_analysis.stats import cliffs_delta_ci, hodges_lehmann, wilson_interval


def test_hodges_lehmann_is_the_median_of_walsh_averages() -> None:
    estimate, low, high = hodges_lehmann(np.array([1.0, 2.0, 3.0]))
    assert estimate == 2.0
    assert low <= estimate <= high


def test_hodges_lehmann_ignores_one_wild_run() -> None:
    differences = np.array([10.0] * 29 + [10_000.0])
    estimate, low, high = hodges_lehmann(differences)
    assert estimate == 10.0
    assert (low, high) == (10.0, 10.0)


def test_cliffs_delta_interval_is_degenerate_for_separated_samples() -> None:
    low, high = cliffs_delta_ci(np.arange(30.0) + 100, np.arange(30.0))
    assert (low, high) == (1.0, 1.0)


def test_cliffs_delta_interval_covers_zero_for_identical_samples() -> None:
    values = np.arange(30.0)
    low, high = cliffs_delta_ci(values, values.copy())
    assert low < 0 < high


def test_wilson_interval_matches_the_closed_form() -> None:
    low, high = wilson_interval(0, 30)
    assert low == 0.0
    assert high == pytest.approx(0.11351, abs=1e-5)
    low, high = wilson_interval(15, 30)
    assert (low, high) == pytest.approx((0.33154, 0.66846), abs=1e-5)


def test_estimators_reject_empty_input() -> None:
    with pytest.raises(ValueError):
        hodges_lehmann(np.array([]))
    with pytest.raises(ValueError):
        cliffs_delta_ci(np.array([]), np.array([1.0]))
    with pytest.raises(ValueError):
        wilson_interval(0, 0)
