"""Statistical analysis functions for WAFER evaluation."""

import numpy as np


def _require_samples(*samples: np.ndarray) -> None:
    if any(len(sample) == 0 for sample in samples):
        raise ValueError("statistical estimators require non-empty samples")


def bootstrap_ci(
    data: np.ndarray, n_resamples: int = 10000, ci: float = 0.95
) -> tuple[float, float]:
    """Bootstrap confidence interval for the median.

    Returns (lower_bound, upper_bound).
    """
    _require_samples(data)
    rng = np.random.default_rng(42)
    medians = np.array(
        [
            np.median(rng.choice(data, size=len(data), replace=True))
            for _ in range(n_resamples)
        ]
    )
    alpha = (1 - ci) / 2
    lower = float(np.percentile(medians, alpha * 100))
    upper = float(np.percentile(medians, (1 - alpha) * 100))
    return lower, upper


def cliffs_delta(a: np.ndarray, b: np.ndarray) -> tuple[float, str]:
    """Cliff's Delta effect size (non-parametric).

    Returns (delta, magnitude) where magnitude is one of:
    'negligible', 'small', 'medium', 'large'.
    """
    _require_samples(a, b)
    n_a, n_b = len(a), len(b)
    # Count dominance pairs
    more = sum(1 for x in a for y in b if x > y)
    less = sum(1 for x in a for y in b if x < y)
    delta = (more - less) / (n_a * n_b)

    # Magnitude thresholds (Romano et al. 2006)
    abs_delta = abs(delta)
    if abs_delta < 0.147:
        magnitude = "negligible"
    elif abs_delta < 0.33:
        magnitude = "small"
    elif abs_delta < 0.474:
        magnitude = "medium"
    else:
        magnitude = "large"

    return float(delta), magnitude



def _percentile_interval(estimates: np.ndarray, ci: float) -> tuple[float, float]:
    alpha = (1 - ci) / 2
    return (
        float(np.percentile(estimates, alpha * 100)),
        float(np.percentile(estimates, (1 - alpha) * 100)),
    )


def _cliffs_delta_value(a: np.ndarray, b: np.ndarray) -> float:
    return float(np.mean(np.sign(a[:, None] - b[None, :])))


def cliffs_delta_ci(
    a: np.ndarray, b: np.ndarray, n_resamples: int = 10000, ci: float = 0.95, seed: int = 42
) -> tuple[float, float]:
    """Percentile bootstrap interval for Cliff's delta, resampling each group independently."""
    _require_samples(a, b)
    a, b = np.asarray(a, dtype=float), np.asarray(b, dtype=float)
    rng = np.random.default_rng(seed)
    deltas = np.array(
        [
            _cliffs_delta_value(rng.choice(a, size=len(a)), rng.choice(b, size=len(b)))
            for _ in range(n_resamples)
        ]
    )
    return _percentile_interval(deltas, ci)


def _walsh_median(values: np.ndarray) -> float:
    upper = np.triu_indices(len(values))
    return float(np.median((values[:, None] + values[None, :])[upper] / 2))


def hodges_lehmann(
    differences: np.ndarray, n_resamples: int = 10000, ci: float = 0.95, seed: int = 42
) -> tuple[float, float, float]:
    """One-sample Hodges-Lehmann shift of paired differences with a percentile bootstrap interval.

    Returns (estimate, lower_bound, upper_bound).
    """
    _require_samples(differences)
    differences = np.asarray(differences, dtype=float)
    rng = np.random.default_rng(seed)
    shifts = np.array(
        [
            _walsh_median(rng.choice(differences, size=len(differences)))
            for _ in range(n_resamples)
        ]
    )
    return (_walsh_median(differences), *_percentile_interval(shifts, ci))


def pooled_ratio_ci(
    numerators: np.ndarray,
    denominators: np.ndarray,
    n_resamples: int = 10000,
    ci: float = 0.95,
    seed: int = 42,
) -> tuple[float, float]:
    """Percentile bootstrap interval for sum(numerators) / sum(denominators), resampling runs.

    Runs are the independent unit, so a burst of loss inside one run widens the
    interval instead of counting as thousands of independent trials.
    """
    _require_samples(numerators, denominators)
    numerators = np.asarray(numerators, dtype=float)
    denominators = np.asarray(denominators, dtype=float)
    if numerators.shape != denominators.shape or np.any(denominators <= 0):
        raise ValueError("a pooled ratio needs one positive denominator per numerator")
    rng = np.random.default_rng(seed)
    indices = rng.integers(0, len(numerators), size=(n_resamples, len(numerators)))
    ratios = numerators[indices].sum(axis=1) / denominators[indices].sum(axis=1)
    return _percentile_interval(ratios, ci)
