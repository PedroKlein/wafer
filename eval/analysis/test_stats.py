from __future__ import annotations

import numpy as np
import pytest

from wafer_analysis.stats import cliffs_delta_ci, hodges_lehmann, pooled_ratio_ci


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


def test_pooled_loss_interval_treats_a_burst_as_one_run() -> None:
    lost = np.array([3_000] + [0] * 29)
    offered = np.full(30, 60_000)
    low, high = pooled_ratio_ci(lost, offered)
    assert low == 0.0
    assert high > 0.004


def test_pooled_loss_interval_is_exact_when_every_run_agrees() -> None:
    low, high = pooled_ratio_ci(np.full(30, 60), np.full(30, 60_000))
    assert (low, high) == pytest.approx((0.001, 0.001))


def test_estimators_reject_empty_input() -> None:
    with pytest.raises(ValueError):
        hodges_lehmann(np.array([]))
    with pytest.raises(ValueError):
        cliffs_delta_ci(np.array([]), np.array([1.0]))
    with pytest.raises(ValueError):
        pooled_ratio_ci(np.array([1.0]), np.array([0.0]))
