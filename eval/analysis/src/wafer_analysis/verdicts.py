"""Thresholds declared in the canonical matrix and the one rule that turns bounds into verdicts.

A criterion with a threshold is ``PASS`` only when the one-sided bound on its
favourable side meets the threshold, ``FAIL`` only when the bound on the other
side misses it, and ``INCONCLUSIVE`` otherwise. Both bounds come from one
run-level bootstrap interval whose ends are the declared one-sided level, so a
95% one-sided bound is an end of the two-sided 90% interval. ``PENDING`` marks
a criterion without a population to bound. Zero-tolerance counts are compared
exactly and never get an interval.
"""

from __future__ import annotations

import json
import math
import operator
from dataclasses import dataclass
from pathlib import Path

from . import paths

VERDICTS = ("PASS", "FAIL", "INCONCLUSIVE", "PENDING")
_COMPARE = {"<": operator.lt, "<=": operator.le, ">": operator.gt, ">=": operator.ge}
_FIELDS = (
    "criterion",
    "experiments",
    "value",
    "unit",
    "direction",
    "statistic",
    "rule",
    "role",
    "origin",
    "pilot_data_visible",
)
VERDICT_SUFFIXES = ("verdict", "threshold", "flips_at", "ci_half_width")


@dataclass(frozen=True)
class Threshold:
    criterion: str
    value: float
    unit: str
    direction: str
    rule: str
    one_sided_confidence: float

    def holds(self, estimate: float) -> bool:
        return _COMPARE[self.direction](estimate, self.value)

    @property
    def interval(self) -> float:
        """Two-sided level whose two ends are the one-sided bounds."""
        return round(2 * self.one_sided_confidence - 1, 12)

    @property
    def lower_is_better(self) -> bool:
        return self.direction in {"<", "<="}


def declared_thresholds(matrix_path: Path | None = None) -> dict[str, Threshold]:
    """Every threshold of ``verdict_rules`` in ``eval/canonical-matrix.json``, by criterion."""
    path = matrix_path or paths._find_repo_root() / "eval/canonical-matrix.json"
    rules = json.loads(Path(path).read_text()).get("verdict_rules")
    if not isinstance(rules, dict) or not isinstance(rules.get("thresholds"), list):
        raise ValueError(f"{path} declares no verdict_rules threshold table")
    confidence = rules.get("one_sided_confidence")
    if type(confidence) is not float or not 0.5 < confidence < 1:
        raise ValueError("verdict_rules one_sided_confidence must lie between 0.5 and 1")
    declared: dict[str, Threshold] = {}
    for row in rules["thresholds"]:
        if not isinstance(row, dict) or set(row) != set(_FIELDS):
            raise ValueError(f"malformed verdict threshold: {row!r}")
        value = row["value"]
        if (
            type(value) not in (int, float)
            or not math.isfinite(value)
            or row["direction"] not in _COMPARE
            or row["criterion"] in declared
        ):
            raise ValueError(f"malformed verdict threshold: {row['criterion']!r}")
        declared[row["criterion"]] = Threshold(
            row["criterion"], value, row["unit"], row["direction"], row["rule"], confidence
        )
    return declared


def bound_verdict(
    prefix: str, threshold: Threshold, bounds: tuple[float, float] | None
) -> dict:
    """Verdict columns for one criterion from its one-sided lower and upper bounds.

    ``flips_at`` is the bound the threshold would have to cross to change the
    verdict: the favourable bound for ``PASS``, the other bound for ``FAIL``,
    and the nearer bound for ``INCONCLUSIVE``. ``ci_half_width`` is half the
    distance between the two bounds.
    """
    if bounds is None:
        return {
            f"{prefix}_verdict": "PENDING",
            f"{prefix}_threshold": threshold.value,
            f"{prefix}_flips_at": None,
            f"{prefix}_ci_half_width": None,
        }
    low, high = (float(bound) for bound in bounds)
    favourable, other = (high, low) if threshold.lower_is_better else (low, high)
    if threshold.holds(favourable):
        verdict, flips_at = "PASS", favourable
    elif not threshold.holds(other):
        verdict, flips_at = "FAIL", other
    else:
        verdict = "INCONCLUSIVE"
        flips_at = min((low, high), key=lambda bound: abs(bound - threshold.value))
    return {
        f"{prefix}_verdict": verdict,
        f"{prefix}_threshold": threshold.value,
        f"{prefix}_flips_at": flips_at,
        f"{prefix}_ci_half_width": (high - low) / 2,
    }


def count_verdict(threshold: Threshold, violations: int) -> str:
    """``PASS`` when an exact count meets its zero-tolerance threshold, else ``FAIL``."""
    return "PASS" if threshold.holds(violations) else "FAIL"


def combined_verdict(*verdicts: str | None) -> str:
    """One verdict over several criteria: any ``FAIL`` fails, then ``PENDING``, then ``INCONCLUSIVE``."""
    present = {verdict for verdict in verdicts if verdict is not None}
    for verdict in ("FAIL", "PENDING", "INCONCLUSIVE"):
        if verdict in present:
            return verdict
    return "PASS"


def no_verdict(prefix: str) -> dict:
    """Empty verdict columns for a row the criterion does not apply to."""
    return dict.fromkeys(f"{prefix}_{suffix}" for suffix in VERDICT_SUFFIXES)
