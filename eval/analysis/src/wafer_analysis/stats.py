"""Statistical analysis functions for WAFER evaluation."""

import numpy as np
from scipy import stats as scipy_stats


def mann_whitney_u(a: np.ndarray, b: np.ndarray) -> tuple[float, float]:
    """Mann-Whitney U test for non-normal distributions.
    
    Returns (U statistic, p-value).
    """
    u_stat, p_value = scipy_stats.mannwhitneyu(a, b, alternative="two-sided")
    return float(u_stat), float(p_value)


def bootstrap_ci(
    data: np.ndarray, n_resamples: int = 10000, ci: float = 0.95
) -> tuple[float, float]:
    """Bootstrap confidence interval for the median.
    
    Returns (lower_bound, upper_bound).
    """
    rng = np.random.default_rng(42)
    medians = np.array([
        np.median(rng.choice(data, size=len(data), replace=True))
        for _ in range(n_resamples)
    ])
    alpha = (1 - ci) / 2
    lower = float(np.percentile(medians, alpha * 100))
    upper = float(np.percentile(medians, (1 - alpha) * 100))
    return lower, upper


def cliffs_delta(a: np.ndarray, b: np.ndarray) -> tuple[float, str]:
    """Cliff's Delta effect size (non-parametric).
    
    Returns (delta, magnitude) where magnitude is one of:
    'negligible', 'small', 'medium', 'large'.
    """
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


def shapiro_wilk(data: np.ndarray) -> tuple[float, float, bool]:
    """Shapiro-Wilk normality test.
    
    Returns (statistic, p_value, is_normal) where is_normal is True if p > 0.05.
    """
    stat, p_value = scipy_stats.shapiro(data)
    return float(stat), float(p_value), p_value > 0.05
