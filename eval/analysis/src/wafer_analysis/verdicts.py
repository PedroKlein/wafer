"""Thresholds declared in the canonical matrix and the one rule that turns bounds into verdicts.

A criterion with a threshold is ``PASS`` only when the one-sided bound on its
favourable side meets the threshold, ``FAIL`` only when the bound on the other
side misses it, and ``INCONCLUSIVE`` otherwise. Both bounds come from one
run-level bootstrap interval whose ends are the declared one-sided level, so a
95% one-sided bound is an end of the two-sided 90% interval. ``PENDING`` marks
a criterion without a population to bound. Zero-tolerance counts are compared
exactly and never get an interval.

Replication hosts are compared with the canonical host criterion by criterion
under the matrix's ``replication_concordance`` rule; the comparison never
changes a canonical verdict.
"""

from __future__ import annotations

import json
import math
import operator
from collections.abc import Mapping
from dataclasses import dataclass
from pathlib import Path

import pandas as pd

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
VERDICT_SUFFIXES = ("verdict", "estimate", "threshold", "flips_at", "ci_half_width")


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


def _matrix(matrix_path: Path | None) -> tuple[Path, dict]:
    path = Path(matrix_path or paths._find_repo_root() / "eval/canonical-matrix.json")
    return path, json.loads(path.read_text())


def declared_thresholds(matrix_path: Path | None = None) -> dict[str, Threshold]:
    """Every threshold of ``verdict_rules`` in ``eval/canonical-matrix.json``, by criterion."""
    path, matrix = _matrix(matrix_path)
    rules = matrix.get("verdict_rules")
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
    prefix: str,
    threshold: Threshold,
    bounds: tuple[float, float] | None,
    *,
    estimate: float | None,
) -> dict:
    """Verdict columns for one criterion from its one-sided lower and upper bounds.

    ``estimate`` is the point estimate of the bounded statistic. ``flips_at``
    is the bound the threshold would have to cross to change the verdict: the
    favourable bound for ``PASS``, the other bound for ``FAIL``, and the
    nearer bound for ``INCONCLUSIVE``. ``ci_half_width`` is half the distance
    between the two bounds.
    """
    if bounds is None:
        return {
            f"{prefix}_verdict": "PENDING",
            f"{prefix}_estimate": _number(estimate),
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
        f"{prefix}_estimate": _number(estimate),
        f"{prefix}_threshold": threshold.value,
        f"{prefix}_flips_at": flips_at,
        f"{prefix}_ci_half_width": (high - low) / 2,
    }


def _number(value: float | None) -> float | None:
    return None if value is None else float(value)


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


CONCORDANCE = ("not-estimable", "same-verdict", "same-direction", "opposite-direction")
_DECIDED = {"PASS", "FAIL", "INCONCLUSIVE"}


@dataclass(frozen=True)
class Concordance:
    canonical_host: str
    replication_hosts: tuple[str, ...]
    criteria_rule: str


def declared_concordance(matrix_path: Path | None = None) -> Concordance:
    """The ``replication_concordance`` rule of ``eval/canonical-matrix.json``.

    The classes must be the ones :func:`concordance` applies, in its order.
    """
    path, matrix = _matrix(matrix_path)
    rule = matrix.get("replication_concordance")
    if not isinstance(rule, dict):
        raise ValueError(f"{path} declares no replication_concordance rule")
    classes = rule.get("classes")
    if not isinstance(classes, list) or tuple(
        row.get("class") if isinstance(row, dict) else None for row in classes
    ) != CONCORDANCE:
        raise ValueError(f"replication_concordance classes differ from {CONCORDANCE}")
    hosts = rule.get("replication_hosts")
    if (
        not isinstance(rule.get("canonical_host"), str)
        or not isinstance(hosts, list)
        or not all(isinstance(host, str) for host in hosts)
        or not isinstance(rule.get("criteria_rule"), str)
    ):
        raise ValueError(f"malformed replication_concordance rule in {path}")
    return Concordance(rule["canonical_host"], tuple(hosts), rule["criteria_rule"])


def _estimable(verdict: object, estimate: object) -> bool:
    return verdict in _DECIDED and not pd.isna(estimate)


def _side(threshold: Threshold, verdict: object, estimate: object) -> str | None:
    if not _estimable(verdict, estimate):
        return None
    return "meets" if threshold.holds(float(estimate)) else "misses"


def concordance(
    threshold: Threshold,
    canonical_verdict: str | None,
    canonical_estimate: float | None,
    replication_verdict: str | None,
    replication_estimate: float | None,
) -> str:
    """How a replication host's verdict on one criterion agrees with the canonical host's.

    Direction is the side of the threshold on which a point estimate falls.
    """
    canonical_side = _side(threshold, canonical_verdict, canonical_estimate)
    replication_side = _side(threshold, replication_verdict, replication_estimate)
    if canonical_side is None or replication_side is None:
        return "not-estimable"
    if canonical_verdict == replication_verdict:
        return "same-verdict"
    return "same-direction" if canonical_side == replication_side else "opposite-direction"


def _criterion_rows(table: pd.DataFrame | None, prefix: str, key: str) -> dict[object, dict]:
    columns = (f"{prefix}_verdict", f"{prefix}_estimate")
    if table is None or not set(columns) <= set(table.columns):
        return {}
    rows = {}
    for row in table.to_dict("records"):
        if pd.isna(row[columns[0]]):
            continue
        if row[key] in rows:
            raise ValueError(f"{prefix} has two rows for {key} {row[key]!r}")
        rows[row[key]] = {
            "verdict": row[columns[0]],
            "estimate": None if pd.isna(row[columns[1]]) else float(row[columns[1]]),
            "thesis_evidence": row.get("thesis_evidence") is True,
        }
    return rows


def concordance_table(
    tables: Mapping[str, pd.DataFrame | None],
    criteria: Mapping[str, str],
    *,
    key: str = "condition",
    matrix_path: Path | None = None,
) -> pd.DataFrame:
    """Concordance of each replication host with the canonical host, per criterion and row.

    ``tables`` holds one verdict table per host tag, all built by the same
    table builder; a host without a table is missing. ``criteria`` maps each
    column prefix to its declared criterion. Rows are matched by ``key``. The
    canonical verdict is copied, never changed.
    """
    rule = declared_concordance(matrix_path)
    thresholds = declared_thresholds(matrix_path)
    rows = []
    for prefix, criterion in criteria.items():
        threshold = thresholds[criterion]
        if threshold.rule != rule.criteria_rule:
            raise ValueError(f"{criterion} is not a {rule.criteria_rule} criterion")
        canonical = _criterion_rows(tables.get(rule.canonical_host), prefix, key)
        replicas = {
            host: _criterion_rows(tables.get(host), prefix, key) for host in rule.replication_hosts
        }
        scopes = list(canonical)
        scopes += [scope for host in replicas.values() for scope in host if scope not in scopes]
        for scope in scopes:
            pi = canonical.get(scope, {})
            for host, replica in replicas.items():
                other = replica.get(scope, {})
                rows.append(
                    {
                        "criterion": criterion,
                        key: scope,
                        "host": host,
                        "canonical_host": rule.canonical_host,
                        "threshold": threshold.value,
                        "direction": threshold.direction,
                        "canonical_verdict": pi.get("verdict"),
                        "canonical_estimate": pi.get("estimate"),
                        "canonical_side": _side(threshold, pi.get("verdict"), pi.get("estimate")),
                        "replication_verdict": other.get("verdict"),
                        "replication_estimate": other.get("estimate"),
                        "replication_side": _side(
                            threshold, other.get("verdict"), other.get("estimate")
                        ),
                        "concordance": concordance(
                            threshold,
                            pi.get("verdict"),
                            pi.get("estimate"),
                            other.get("verdict"),
                            other.get("estimate"),
                        ),
                        "units": threshold.unit,
                        "estimator": (
                            "each host's own verdict and point estimate; direction is the side of the threshold "
                            "the point estimate falls on; first matching class of not-estimable, same-verdict, "
                            "same-direction, opposite-direction"
                        ),
                        "claim_boundary": (
                            "replication agreement only; the canonical host's verdict stands as decided"
                        ),
                        "thesis_evidence": pi.get("thesis_evidence", False)
                        and other.get("thesis_evidence", False),
                    }
                )
    return pd.DataFrame(rows)

