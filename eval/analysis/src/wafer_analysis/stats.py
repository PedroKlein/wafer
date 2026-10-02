"""Statistical analysis functions for WAFER evaluation."""

import math

import numpy as np


def _require_samples(*samples: np.ndarray) -> None:
    if any(len(sample) == 0 for sample in samples):
        raise ValueError("statistical estimators require non-empty samples")


def bootstrap_ci(
    data: np.ndarray, n_resamples: int = 10000, ci: float = 0.95, *, statistic=np.median
) -> tuple[float, float]:
    """Percentile bootstrap interval for a one-sample statistic, the median by default.

    Returns (lower_bound, upper_bound).
    """
    _require_samples(data)
    rng = np.random.default_rng(42)
    estimates = np.array(
        [
            statistic(rng.choice(data, size=len(data), replace=True))
            for _ in range(n_resamples)
        ]
    )
    return _percentile_interval(estimates, ci)


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


def _pair_indices(
    rng: np.random.Generator, a: np.ndarray, b: np.ndarray, n_resamples: int
) -> np.ndarray:
    """Indices that resample ``a[i]`` and ``b[i]`` together, one row per resample.

    An effect the two members of a pair share then cancels instead of
    widening the interval.
    """
    if len(a) != len(b):
        raise ValueError("paired resampling needs one b value per a value")
    return rng.integers(0, len(a), size=(n_resamples, len(a)))


def cliffs_delta_ci(
    a: np.ndarray,
    b: np.ndarray,
    n_resamples: int = 10000,
    ci: float = 0.95,
    seed: int = 42,
    *,
    paired: bool = False,
) -> tuple[float, float]:
    """Percentile bootstrap interval for Cliff's delta.

    Each group is resampled independently, or, with ``paired``, index-matched
    pairs are resampled together.
    """
    _require_samples(a, b)
    a, b = np.asarray(a, dtype=float), np.asarray(b, dtype=float)
    rng = np.random.default_rng(seed)
    if paired:
        samples = ((a[row], b[row]) for row in _pair_indices(rng, a, b, n_resamples))
    else:
        samples = (
            (rng.choice(a, size=len(a)), rng.choice(b, size=len(b))) for _ in range(n_resamples)
        )
    deltas = np.array([_cliffs_delta_value(x, y) for x, y in samples])
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


def _increasing_root(function, low: float = 0.0, high: float = 1.0) -> float:
    for _ in range(200):
        middle = (low + high) / 2
        if function(middle) < 0:
            low = middle
        else:
            high = middle
    return (low + high) / 2


def clopper_pearson(successes: int, trials: int, ci: float = 0.95) -> tuple[float, float]:
    """Exact (Clopper-Pearson) interval for a binomial proportion."""
    if trials <= 0 or not 0 <= successes <= trials:
        raise ValueError("a proportion needs 0 <= successes <= trials and at least one trial")
    alpha = (1 - ci) / 2

    def at_least(p: float) -> float:
        return sum(math.comb(trials, i) * p**i * (1 - p) ** (trials - i) for i in range(successes, trials + 1))

    def at_most(p: float) -> float:
        return sum(math.comb(trials, i) * p**i * (1 - p) ** (trials - i) for i in range(successes + 1))

    low = 0.0 if successes == 0 else _increasing_root(lambda p: at_least(p) - alpha)
    high = 1.0 if successes == trials else _increasing_root(lambda p: alpha - at_most(p))
    return low, high


def median_shift_ci(
    a: np.ndarray,
    b: np.ndarray,
    *,
    relative: bool = False,
    paired: bool = False,
    n_resamples: int = 10000,
    ci: float = 0.95,
    seed: int = 42,
) -> tuple[float, float, float]:
    """median(a) - median(b), or that difference over median(b), with a percentile bootstrap interval.

    Each group is resampled independently, which suits unpaired runs. With
    ``paired``, ``a[i]`` and ``b[i]`` ran in the same block and each resample
    draws whole pairs. Returns (estimate, lower_bound, upper_bound).
    """
    _require_samples(a, b)
    a, b = np.asarray(a, dtype=float), np.asarray(b, dtype=float)
    if relative and np.median(b) == 0:
        raise ValueError("a relative shift needs a non-zero reference median")
    rng = np.random.default_rng(seed)
    if paired:
        indices = _pair_indices(rng, a, b, n_resamples)
        medians_a = np.median(a[indices], axis=1)
        medians_b = np.median(b[indices], axis=1)
    else:
        medians_a = np.median(rng.choice(a, size=(n_resamples, len(a))), axis=1)
        medians_b = np.median(rng.choice(b, size=(n_resamples, len(b))), axis=1)
    shifts = medians_a - medians_b
    estimate = float(np.median(a) - np.median(b))
    if relative:
        shifts = shifts / medians_b
        estimate /= float(np.median(b))
    return (estimate, *_percentile_interval(shifts, ci))


def _ols(x: np.ndarray, y: np.ndarray) -> tuple[float, float]:
    slope, intercept = np.polyfit(x, y, 1)
    return float(slope), float(intercept)


def stratified_slope(
    x: np.ndarray, y: np.ndarray, n_resamples: int = 10000, ci: float = 0.95, seed: int = 42
) -> tuple[float, float, float, float, float]:
    """OLS slope of y on x with a percentile bootstrap interval that resamples within each x.

    Every x is a fixed design level holding independent runs, so each resample
    keeps the same number of runs per level. Returns
    (slope, lower_bound, upper_bound, intercept, r_squared).
    """
    _require_samples(x, y)
    x, y = np.asarray(x, dtype=float), np.asarray(y, dtype=float)
    levels = np.unique(x)
    if x.shape != y.shape or len(levels) < 2:
        raise ValueError("a slope needs one value per run and at least two levels")
    slope, intercept = _ols(x, y)
    residual = y - (slope * x + intercept)
    total = np.sum((y - y.mean()) ** 2)
    r_squared = 1.0 - float(np.sum(residual**2) / total) if total else 1.0
    rng = np.random.default_rng(seed)
    groups = [np.flatnonzero(x == level) for level in levels]
    slopes = np.empty(n_resamples)
    for index in range(n_resamples):
        chosen = np.concatenate([rng.choice(group, size=len(group)) for group in groups])
        slopes[index] = _ols(x[chosen], y[chosen])[0]
    return (slope, *_percentile_interval(slopes, ci), intercept, r_squared)
