from __future__ import annotations

import numpy as np
import pytest

from wafer_analysis.stats import (
    clopper_pearson,
    cliffs_delta_ci,
    hodges_lehmann,
    median_shift_ci,
    pooled_ratio_ci,
    stratified_slope,
)


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


def test_clopper_pearson_matches_the_exact_closed_forms() -> None:
    low, high = clopper_pearson(30, 30)
    assert low == pytest.approx(0.025 ** (1 / 30), abs=1e-9)
    assert high == 1.0
    low, high = clopper_pearson(0, 30)
    assert (low, high) == (0.0, pytest.approx(1 - 0.025 ** (1 / 30), abs=1e-9))
    assert clopper_pearson(15, 30) == pytest.approx((0.3129703, 0.6870297), abs=1e-7)


def test_clopper_pearson_rejects_impossible_counts() -> None:
    with pytest.raises(ValueError):
        clopper_pearson(0, 0)
    with pytest.raises(ValueError):
        clopper_pearson(31, 30)


def test_median_shift_is_exact_for_constant_groups() -> None:
    assert median_shift_ci(np.full(30, 990.0), np.full(30, 1_000.0)) == pytest.approx((-10, -10, -10))
    assert median_shift_ci(np.full(30, 990.0), np.full(30, 1_000.0), relative=True) == pytest.approx(
        (-0.01, -0.01, -0.01)
    )


def test_median_shift_interval_covers_zero_for_one_population() -> None:
    values = np.random.default_rng(3).normal(1_000, 10, 60)
    estimate, low, high = median_shift_ci(values[:30], values[30:])
    assert low < 0 < high and low <= estimate <= high


def test_relative_median_shift_needs_a_nonzero_reference() -> None:
    with pytest.raises(ValueError, match="non-zero reference"):
        median_shift_ci(np.ones(3), np.zeros(3), relative=True)


def test_stratified_slope_recovers_an_exact_line() -> None:
    depths = np.repeat([1, 3, 5, 10], 30)
    slope, low, high, intercept, r_squared = stratified_slope(depths, 40.0 + 7.5 * depths)
    assert (slope, low, high, intercept, r_squared) == pytest.approx((7.5, 7.5, 7.5, 40.0, 1.0))


def test_stratified_slope_interval_covers_the_true_slope_under_noise() -> None:
    depths = np.repeat([1, 3, 5, 10], 30)
    values = 40.0 + 7.5 * depths + np.random.default_rng(5).normal(0, 4, len(depths))
    slope, low, high, _, r_squared = stratified_slope(depths, values)
    assert low < 7.5 < high and low < slope < high
    assert 0.8 < r_squared < 1.0


def test_stratified_slope_needs_two_levels() -> None:
    with pytest.raises(ValueError, match="two levels"):
        stratified_slope(np.ones(5), np.arange(5.0))
