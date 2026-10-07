"""Run-level summaries for final canonical evaluation artifacts."""

from __future__ import annotations

import hashlib
import math
import statistics
from collections.abc import Iterable

import numpy as np
import pandas as pd

from .attempts import INCOMPLETE_RUN_REASONS
from .backpressure import BACKPRESSURE_POLICIES, validate_backpressure_result
from .rollback import validate_swap5_artifacts
from .stats import (
    bootstrap_ci,
    cliffs_delta,
    cliffs_delta_ci,
    clopper_pearson,
    hodges_lehmann,
    median_shift_ci,
    pooled_ratio_ci,
    stratified_slope,
)
from .verdicts import (
    bound_verdict,
    combined_verdict,
    count_verdict,
    declared_thresholds,
    no_verdict,
)

def _require_runs(
    records: list[dict], conditions: tuple[str, ...]
) -> dict[str, list[dict]]:
    grouped = {condition: [] for condition in conditions}
    for record in records:
        condition = record.get("condition")
        if condition not in grouped:
            raise ValueError(f"unexpected canonical condition: {condition}")
        grouped[str(condition)].append(record)
    for condition, runs in grouped.items():
        indices = {int(run.get("run_index", 0)) for run in runs}
        if len(runs) != 30 or indices != set(range(1, 31)):
            raise ValueError(f"{condition} requires 30 independent runs")
        runs.sort(key=lambda run: int(run["run_index"]))
    return grouped


def _ci(values: list[float]) -> tuple[float, float]:
    return bootstrap_ci(np.asarray(values, dtype=float))


def _stopped_early(record: dict) -> bool:
    """Whether the system under test ended this admitted run before its evidence was complete."""
    return bool(INCOMPLETE_RUN_REASONS & set(record.get("sut_outcome_reasons", ())))


def _median(values: list[float]) -> float | None:
    return float(np.median(values)) if values else None


def _number(value: float | None) -> float | None:
    return None if value is None else float(value)


def _nearest_rank_p95(values) -> float:
    ordered = np.sort(np.asarray(values, dtype=float))
    return float(ordered[math.ceil(len(ordered) * 0.95) - 1])


TARGET_LOAD_CRITERIA = {
    "p95_ratio": "e-perf-1-p95-ratio",
    "loss": "e-perf-1-pooled-loss",
    "achieved_ratio": "e-perf-1-achieved-ratio",
    "duplicates": "e-perf-1-duplicates",
}


def target_latency_table(records: list[dict], *, canonical: bool = True) -> pd.DataFrame:
    """E-Perf-1 latency and delivery per system at the matched 1,000 msg/s target load.

    The three systems run in one randomised block per run index, so every
    comparison with eKuiper pairs each run with the eKuiper run of the same
    index and resamples those pairs.
    """
    conditions = ("wafer", "native", "ekuiper")
    grouped = _group_runs(records, conditions, canonical=canonical)
    if not all(grouped.values()):
        raise ValueError("target load needs runs of every system")
    rules = declared_thresholds()
    ratio_rule = rules["e-perf-1-p95-ratio"]
    loss_rule = rules["e-perf-1-pooled-loss"]
    achieved_rule = rules["e-perf-1-achieved-ratio"]
    duplicate_rule = rules["e-perf-1-duplicates"]
    reference = {int(run["run_index"]): float(run["p95_ns"]) for run in grouped["ekuiper"]}
    rows = []
    for condition in conditions:
        condition_runs = grouped[condition]
        required_delivery = {
            "intended_messages",
            "received_unique",
            "loss_fraction",
            "achieved_rate_msg_s",
            "achieved_ratio",
            "duplicates",
        }
        if any(not required_delivery <= run.keys() for run in condition_runs):
            raise ValueError(f"{condition} lacks target-load delivery evidence")
        values = np.asarray([run["p95_ns"] for run in condition_runs], dtype=float)
        achieved = np.asarray(
            [run["achieved_rate_msg_s"] for run in condition_runs], dtype=float
        )
        low, high = bootstrap_ci(values)
        achieved_low, achieved_high = bootstrap_ci(achieved)
        pairs = sorted(reference.keys() & {int(run["run_index"]) for run in condition_runs})
        if not pairs:
            raise ValueError(f"{condition} shares no run index with ekuiper")
        p95 = {int(run["run_index"]): float(run["p95_ns"]) for run in condition_runs}
        paired = np.asarray([p95[index] for index in pairs])
        paired_reference = np.asarray([reference[index] for index in pairs])
        shift, shift_low, shift_high = median_shift_ci(
            paired, paired_reference, relative=True, paired=True
        )
        delta, magnitude = cliffs_delta(paired, paired_reference)
        delta_low, delta_high = cliffs_delta_ci(paired, paired_reference, paired=True)
        intended = sum(int(run["intended_messages"]) for run in condition_runs)
        received = sum(int(run["received_unique"]) for run in condition_runs)
        pooled_loss = (intended - received) / intended
        lost = [max(0, int(run["intended_messages"]) - int(run["received_unique"])) for run in condition_runs]
        offered = [int(run["intended_messages"]) for run in condition_runs]
        loss_low, loss_high = pooled_ratio_ci(lost, offered)
        achieved_ratios = np.asarray([run["achieved_ratio"] for run in condition_runs], dtype=float)
        mean_achieved_ratio = float(np.mean(achieved_ratios))
        total_duplicates = sum(int(run["duplicates"]) for run in condition_runs)
        delivery = {
            **bound_verdict(
                "loss",
                loss_rule,
                pooled_ratio_ci(lost, offered, ci=loss_rule.interval),
                estimate=pooled_loss,
            ),
            **bound_verdict(
                "achieved_ratio",
                achieved_rule,
                bootstrap_ci(achieved_ratios, ci=achieved_rule.interval, statistic=np.mean),
                estimate=mean_achieved_ratio,
            ),
            "duplicates_verdict": count_verdict(duplicate_rule, total_duplicates),
            "duplicates_estimate": total_duplicates,
            "duplicates_threshold": duplicate_rule.value,
        }
        delivery["delivery_verdict"] = combined_verdict(
            delivery["loss_verdict"],
            delivery["achieved_ratio_verdict"],
            delivery["duplicates_verdict"],
        )
        if condition == "wafer":
            _, ratio_low, ratio_high = median_shift_ci(
                paired, paired_reference, relative=True, paired=True, ci=ratio_rule.interval
            )
            ratio = bound_verdict(
                "p95_ratio", ratio_rule, (1 + ratio_low, 1 + ratio_high), estimate=1 + shift
            )
            verdict = combined_verdict(ratio["p95_ratio_verdict"], delivery["delivery_verdict"])
        else:
            ratio, verdict = no_verdict("p95_ratio"), None
        rows.append(
            {
                "condition": condition,
                "N_runs": len(values),
                "median_p50_ns": float(
                    np.median([run["p50_ns"] for run in condition_runs])
                ),
                "median_p95_ns": float(np.median(values)),
                "median_p99_ns": float(
                    np.median([run["p99_ns"] for run in condition_runs])
                ),
                "ci95_low_ns": low,
                "ci95_high_ns": high,
                "median_achieved_rate_msg_s": float(np.median(achieved)),
                "achieved_ci95_low_msg_s": achieved_low,
                "achieved_ci95_high_msg_s": achieved_high,
                "pooled_loss": pooled_loss,
                "pooled_loss_ci95_low": loss_low,
                "pooled_loss_ci95_high": loss_high,
                "mean_achieved_ratio": mean_achieved_ratio,
                "total_duplicates": total_duplicates,
                "reference_condition": "ekuiper",
                "median_ratio_vs_reference": 1 + shift,
                "ratio_ci95_low": 1 + shift_low,
                "ratio_ci95_high": 1 + shift_high,
                "cliffs_delta_vs_reference": delta,
                "cliffs_delta_ci95_low": delta_low,
                "cliffs_delta_ci95_high": delta_high,
                "effect_magnitude": magnitude,
                **ratio,
                **delivery,
                "verdict": verdict,
                "units": "nanoseconds, messages/second, fraction, messages",
                "estimator": "median run p95 and achieved rate with bootstrap 95% CI; pooled loss with a run-resampling bootstrap 95% CI; mean achieved ratio; ratio of median run p95 to eKuiper's and Cliff's delta against eKuiper with bootstrap 95% CIs over run pairs, pairing runs by index within the randomised block; verdicts from one-sided 95% bounds, resampling run pairs for the p95 ratio and runs for delivery",
                "threshold": (
                    f"one-sided 95% bounds: median(WAFER p95) / median(eKuiper p95) <= {ratio_rule.value:g}; "
                    f"pooled loss <= {loss_rule.value:g}; mean achieved/offered >= {achieved_rule.value:g}; zero duplicates"
                ),
                "claim_boundary": "matched 1,000 msg/s target load; not capacity",
                "thesis_evidence": canonical,
            }
        )
    return pd.DataFrame(rows)


MDD_ALPHA = 0.05
MDD_POWER = 0.80
_MDD_Z = statistics.NormalDist().inv_cdf(1 - MDD_ALPHA / 2) + statistics.NormalDist().inv_cdf(
    MDD_POWER
)


def _wafer_native_contrast(
    records: list[dict],
    conditions: tuple[str, ...],
    percentiles: tuple[str, ...],
    *,
    canonical: bool,
    claim_boundary: str,
) -> pd.DataFrame:
    grouped = _group_runs(records, conditions, canonical=canonical)
    arms = [grouped["wafer"], grouped["native"]]
    stopped = sum(_stopped_early(run) for runs in arms for run in runs)
    wafer_runs, native_runs = (
        {int(run["run_index"]): run for run in runs if not _stopped_early(run)} for runs in arms
    )
    pairs = sorted(wafer_runs.keys() & native_runs.keys())
    if not pairs:
        return pd.DataFrame()
    rows = []
    for percentile in percentiles:
        wafer = np.asarray([wafer_runs[index][f"{percentile}_ns"] for index in pairs], dtype=float)
        native = np.asarray([native_runs[index][f"{percentile}_ns"] for index in pairs], dtype=float)
        difference = wafer - native
        low, high = bootstrap_ci(difference)
        shift, shift_low, shift_high = median_shift_ci(wafer, native, relative=True, paired=True)
        paired_sd = float(np.std(difference, ddof=1)) if len(pairs) > 1 else None
        rows.append(
            {
                "statistic": percentile,
                "condition": "wafer",
                "reference_condition": "native",
                "N_pairs": len(pairs),
                "runs_stopped_early": stopped,
                "wafer_median_ns": float(np.median(wafer)),
                "native_median_ns": float(np.median(native)),
                "median_ratio": 1 + shift,
                "ratio_ci95_low": 1 + shift_low,
                "ratio_ci95_high": 1 + shift_high,
                "difference_ns": float(np.median(difference)),
                "difference_ci95_low_ns": low,
                "difference_ci95_high_ns": high,
                "difference_ci_half_width_ns": (high - low) / 2,
                "paired_sd_ns": paired_sd,
                "mdd_ns": None if paired_sd is None else _MDD_Z * paired_sd / math.sqrt(len(pairs)),
                "units": "nanoseconds, ratio, run pairs",
                "estimator": (
                    f"WAFER and native run {percentile} paired by run index within the randomised block, "
                    "leaving out runs the system under test stopped early and their partners; "
                    "median over pairs of WAFER minus native with a bootstrap 95% CI over pairs and its half-width; "
                    "median WAFER over median native with a bootstrap 95% CI over pairs; "
                    "minimum detectable difference (z(0.975) + z(0.80)) x paired SD / sqrt(N pairs), "
                    f"two-sided alpha {MDD_ALPHA:g} and power {MDD_POWER:.2f}, normal approximation"
                ),
                "uncertainty": "bootstrap 95% CIs over run pairs",
                "threshold": "none: descriptive contrast without a verdict",
                "claim_boundary": claim_boundary,
                "thesis_evidence": canonical,
            }
        )
    return pd.DataFrame(rows)


def target_contrast_table(records: list[dict], *, canonical: bool = True) -> pd.DataFrame:
    """E-Perf-1: paired WAFER minus native run p95 and p50 at the matched target load.

    Each record is one run's ``p95_ns`` and ``p50_ns``; eKuiper runs may be
    present and are not part of the contrast.
    """
    return _wafer_native_contrast(
        records,
        ("wafer", "native", "ekuiper"),
        ("p95", "p50"),
        canonical=canonical,
        claim_boundary="matched 1,000 msg/s target load over MQTT; descriptive, no verdict",
    )


def overhead_contrast_table(records: list[dict], *, canonical: bool = True) -> pd.DataFrame:
    """E-Perf-5 on one host: paired WAFER/native ratio and WAFER minus native run p50."""
    return _wafer_native_contrast(
        records,
        ("wafer", "native"),
        ("p50",),
        canonical=canonical,
        claim_boundary="in-process path on one host; descriptive, no verdict; no cross-architecture claim",
    )


def metering_table(records: list[dict]) -> pd.DataFrame:
    conditions = ("neither", "fuel-only", "epoch-only", "both")
    grouped = _require_runs(records, conditions)
    reference = np.asarray([run["p95_ns"] for run in grouped["neither"]], dtype=float)
    rows = []
    for condition in conditions:
        values = np.asarray([run["p95_ns"] for run in grouped[condition]], dtype=float)
        low, high = bootstrap_ci(values)
        delta, magnitude = cliffs_delta(values, reference)
        delta_low, delta_high = cliffs_delta_ci(values, reference)
        paired_difference = values - reference
        shift, shift_low, shift_high = hodges_lehmann(paired_difference)
        paired_ratio = values / reference
        difference_low, difference_high = bootstrap_ci(paired_difference)
        ratio_low, ratio_high = bootstrap_ci(paired_ratio)
        rows.append(
            {
                "condition": condition,
                "N_runs": len(values),
                "median_p95_ns": float(np.median(values)),
                "ci95_low_ns": low,
                "ci95_high_ns": high,
                "reference_condition": "neither",
                "median_p95_difference_ns": float(np.median(paired_difference)),
                "difference_ci95_low_ns": difference_low,
                "difference_ci95_high_ns": difference_high,
                "median_p95_ratio": float(np.median(paired_ratio)),
                "ratio_ci95_low": ratio_low,
                "ratio_ci95_high": ratio_high,
                "hodges_lehmann_shift_ns": shift,
                "shift_ci95_low_ns": shift_low,
                "shift_ci95_high_ns": shift_high,
                "cliffs_delta_vs_neither": delta,
                "cliffs_delta_ci95_low": delta_low,
                "cliffs_delta_ci95_high": delta_high,
                "effect_magnitude": magnitude,
                "units": "nanoseconds",
                "estimator": "median run p95 difference and ratio with bootstrap 95% CI; Hodges-Lehmann shift of the paired differences and Cliff's delta, each with bootstrap 95% CI",
                "claim_boundary": "run-level metering ablation; no per-message inference",
                "thesis_evidence": True,
            }
        )
    return pd.DataFrame(rows)


CONTAINMENT_ATTACKS = {
    "e-iso-1": "buffer-overflow",
    "e-iso-2": "cross-read",
    "e-iso-3": "fs-access",
    "e-iso-4": "infinite-loop",
    "e-iso-5": "memory-exhaust",
    "e-iso-6": "panic",
}


def containment_table(records: list[dict], *, canonical: bool = True) -> pd.DataFrame:
    """One row per single-node attack from its runs' containment.json records.

    A canonical table needs all six attacks at 30 runs; ``canonical=False``
    summarises whatever runs exist and marks the rows as non-evidence.
    """
    escapes = declared_thresholds()["e-iso-containment"]
    rows = []
    for experiment, condition in CONTAINMENT_ATTACKS.items():
        runs = [record for record in records if record.get("experiment") == experiment]
        if any(run.get("condition") != condition for run in runs):
            raise ValueError(f"{experiment} holds a condition other than {condition}")
        if len({run.get("run_index") for run in runs}) != len(runs):
            raise ValueError(f"{experiment} contains duplicate run identity")
        if canonical:
            _require_runs(runs, (condition,))
        if not runs:
            continue
        complete = [run for run in runs if not _stopped_early(run)]
        if any(run.get("contained") is None for run in complete):
            raise ValueError(f"{experiment} has a run without a containment verdict")
        contained = sum(bool(run["contained"]) for run in complete)
        low, high = clopper_pearson(contained, len(runs))
        rows.append(
            {
                "experiment": experiment,
                "condition": condition,
                "expected_mechanism": runs[0].get("expected_mechanism"),
                "N_runs": len(runs),
                "contained_runs": contained,
                "containment_proportion": contained / len(runs),
                "containment_ci95_low": low,
                "containment_ci95_high": high,
                "escape_probability_upper975": 1 - low,
                "median_mechanism_count": _median([run["expected_count"] for run in complete]),
                "max_unexpected_outcomes": max(
                    (int(run["unexpected_outcomes"]) for run in complete), default=None
                ),
                "runtime_panics": sum(bool(run["runtime_panic"]) for run in complete),
                "runs_stopped_early": len(runs) - len(complete),
                "median_healthy_messages_out": _median(
                    [run["healthy_messages_out"] for run in complete]
                ),
                "all_contained": escapes.holds(len(runs) - contained),
                "units": "runs, events, messages",
                "estimator": "runs in which the expected mechanism stopped the attack and nothing else happened, with a two-sided Clopper-Pearson 95% interval; its lower end gives the one-sided 97.5% upper bound on the escape probability; a run the runtime did not survive counts as not contained",
                "threshold": "every run contained",
                "claim_boundary": "per attack on this host; the escape bound covers this attack only",
                "thesis_evidence": canonical,
            }
        )
    return pd.DataFrame(rows)


def _group_runs(
    records: list[dict], conditions: tuple[str, ...], *, canonical: bool
) -> dict[str, list[dict]]:
    if canonical:
        return _require_runs(records, conditions)
    grouped = {condition: [] for condition in conditions}
    for record in records:
        if record.get("condition") not in grouped:
            raise ValueError(f"unexpected condition: {record.get('condition')}")
        grouped[record["condition"]].append(record)
    for condition, runs in grouped.items():
        if len({run.get("run_index") for run in runs}) != len(runs):
            raise ValueError(f"{condition} contains duplicate run identity")
        runs.sort(key=lambda run: int(run["run_index"]))
    return grouped


BRANCH_ISOLATION_CONDITIONS = ("control", "panic-attack", "epoch-loop-attack")


def branch_isolation_table(records: list[dict], *, canonical: bool = True) -> pd.DataFrame:
    """Branch-A throughput, p95 and loss per E-Iso-7 condition, contrasted with the control.

    ``records`` are the runs' ``branch-isolation.json`` documents. A run the runtime did not
    survive has no branch measurement; it counts in ``N_runs`` and an attack condition with
    such a run fails its condition.
    """
    grouped = _group_runs(records, BRANCH_ISOLATION_CONDITIONS, canonical=canonical)
    if not grouped["control"]:
        return pd.DataFrame()
    rules = declared_thresholds()
    drop_rule = rules["e-iso-7-throughput-drop"]
    stopped_rule = rules["e-iso-7-stopped-runs"]
    threshold = (
        f"one-sided 95% upper bound of the branch-A throughput drop < {drop_rule.value:g} percent; "
        "no run stopped early"
    )
    admitted = {condition: len(runs) for condition, runs in grouped.items()}
    grouped = {
        condition: [run for run in runs if not _stopped_early(run)]
        for condition, runs in grouped.items()
    }

    def branch_a(run: dict, *keys: str) -> float:
        value = run["branches"]["branch_a"]
        for key in keys:
            value = value[key]
        return float(value)

    def metrics(runs: list[dict]) -> tuple[np.ndarray, np.ndarray]:
        return (
            np.asarray([branch_a(run, "throughput", "mean_messages_per_second") for run in runs]),
            np.asarray([branch_a(run, "latency_ns", "p95") for run in runs]),
        )

    def arrival_span_throughput(runs: list[dict]) -> np.ndarray | None:
        spans = [run.get("branch_a_arrival_span_ns") for run in runs]
        if not runs or not all(spans):
            return None
        return np.asarray(
            [
                branch_a(run, "throughput", "total_messages") * 1e9 / span
                for run, span in zip(runs, spans, strict=True)
            ]
        )

    control_throughput, control_p95 = metrics(grouped["control"])
    control_arrival = arrival_span_throughput(grouped["control"])
    rows = []
    for condition in BRANCH_ISOLATION_CONDITIONS:
        runs = grouped[condition]
        if not admitted[condition]:
            continue
        stopped = admitted[condition] - len(runs)
        if not runs:
            rows.append(
                {
                    "condition": condition,
                    "N_runs": admitted[condition],
                    "runs_stopped_early": stopped,
                    **(
                        no_verdict("drop")
                        if condition == "control"
                        else bound_verdict("drop", drop_rule, None, estimate=None)
                    ),
                    "verdict": None if condition == "control" else "FAIL",
                    "reference_condition": "control",
                    "units": "runs",
                    "estimator": "every run of this condition stopped before its branch measurement",
                    "threshold": threshold,
                    "claim_boundary": "independently sourced branch A on the same runtime; no claim about branch B",
                    "thesis_evidence": canonical,
                }
            )
            continue
        throughput, p95 = metrics(runs)
        arrival = arrival_span_throughput(runs)
        offered = [int(run["branches"]["branch_a"]["offered_messages"]) for run in runs]
        lost = [int(run["branches"]["branch_a"]["lost_messages"]) for run in runs]
        throughput_low, throughput_high = bootstrap_ci(throughput)
        p95_low, p95_high = bootstrap_ci(p95)
        loss_low, loss_high = pooled_ratio_ci(lost, offered)
        row = {
            "condition": condition,
            "N_runs": admitted[condition],
            "runs_stopped_early": stopped,
            "median_throughput_msg_s": float(np.median(throughput)),
            "throughput_ci95_low_msg_s": throughput_low,
            "throughput_ci95_high_msg_s": throughput_high,
            "median_arrival_span_throughput_msg_s": None if arrival is None else float(np.median(arrival)),
            "median_p95_ns": float(np.median(p95)),
            "p95_ci95_low_ns": p95_low,
            "p95_ci95_high_ns": p95_high,
            "pooled_loss": sum(lost) / sum(offered),
            "pooled_loss_ci95_low": loss_low,
            "pooled_loss_ci95_high": loss_high,
            "reference_condition": "control",
            **dict.fromkeys(
                (
                    "throughput_drop_percent",
                    "drop_ci95_low_percent",
                    "drop_ci95_high_percent",
                    "arrival_span_drop_percent",
                    "arrival_span_drop_ci95_low_percent",
                    "arrival_span_drop_ci95_high_percent",
                    "p95_increase_percent",
                    "increase_ci95_low_percent",
                    "increase_ci95_high_percent",
                    "throughput_cliffs_delta",
                    "cliffs_delta_ci95_low",
                    "cliffs_delta_ci95_high",
                    "effect_magnitude",
                    "verdict",
                )
            ),
            **no_verdict("drop"),
        }
        if condition != "control" and not len(control_throughput):
            row.update(bound_verdict("drop", drop_rule, None, estimate=None))
            row["verdict"] = combined_verdict(
                row["drop_verdict"], count_verdict(stopped_rule, stopped)
            )
        elif condition != "control":
            drop, drop_low, drop_high = median_shift_ci(throughput, control_throughput, relative=True)
            _, bound_low, bound_high = median_shift_ci(
                throughput, control_throughput, relative=True, ci=drop_rule.interval
            )
            increase, increase_low, increase_high = median_shift_ci(p95, control_p95, relative=True)
            delta, magnitude = cliffs_delta(throughput, control_throughput)
            delta_low, delta_high = cliffs_delta_ci(throughput, control_throughput)
            row.update(
                {
                    "throughput_drop_percent": -100 * drop,
                    "drop_ci95_low_percent": -100 * drop_high,
                    "drop_ci95_high_percent": -100 * drop_low,
                    "p95_increase_percent": 100 * increase,
                    "increase_ci95_low_percent": 100 * increase_low,
                    "increase_ci95_high_percent": 100 * increase_high,
                    "throughput_cliffs_delta": delta,
                    "cliffs_delta_ci95_low": delta_low,
                    "cliffs_delta_ci95_high": delta_high,
                    "effect_magnitude": magnitude,
                    **bound_verdict(
                        "drop", drop_rule, (-100 * bound_high, -100 * bound_low), estimate=-100 * drop
                    ),
                }
            )
            row["verdict"] = combined_verdict(
                row["drop_verdict"], count_verdict(stopped_rule, stopped)
            )
            if arrival is not None and control_arrival is not None:
                arrival_drop, arrival_low, arrival_high = median_shift_ci(
                    arrival, control_arrival, relative=True
                )
                row.update(
                    {
                        "arrival_span_drop_percent": -100 * arrival_drop,
                        "arrival_span_drop_ci95_low_percent": -100 * arrival_high,
                        "arrival_span_drop_ci95_high_percent": -100 * arrival_low,
                    }
                )
        row.update(
            {
                "units": "messages/second, nanoseconds, percent, fraction",
                "estimator": "branch-A median run throughput and p95 with bootstrap 95% CIs over the runs that completed; drop and increase relative to the control median with a two-group bootstrap 95% CI; Cliff's delta with bootstrap 95% CI; pooled loss with a run-resampling bootstrap 95% CI; verdict from the one-sided 95% upper bound of the drop, resampling each condition's runs apart; a run the runtime did not survive fails its condition; reported only, not used by the verdict: arrival-span throughput, the same messages over the time from the first post-warmup arrival to the end of the last interval row, which leaves out the sink's file export, and its drop against the control computed the same way",
                "threshold": threshold,
                "claim_boundary": "independently sourced branch A on the same runtime; no claim about branch B",
                "thesis_evidence": canonical,
            }
        )
        rows.append(row)
    return pd.DataFrame(rows)


def recovery_table(records: list[dict], *, canonical: bool = True) -> pd.DataFrame:
    """E-Iso-8 trap-to-running durations: per-run statistics over runs, and pooled percentiles.

    Each record carries a run's ``run_index``, ``condition`` and the
    ``durations_ns`` read from its ``recovery.csv``. A run the runtime did not survive has
    no durations; it counts in ``N_runs`` and ``runs_stopped_early``.
    """
    admitted = _group_runs(records, ("panic-recovery",), canonical=canonical)["panic-recovery"]
    if not admitted:
        return pd.DataFrame()
    runs = [run for run in admitted if not _stopped_early(run)]
    if any(not run["durations_ns"] for run in runs):
        raise ValueError("every E-Iso-8 run needs at least one recovery sample")
    summary = {}
    if runs:
        medians = np.asarray([np.median(run["durations_ns"]) for run in runs], dtype=float)
        p95s = np.asarray([np.percentile(run["durations_ns"], 95) for run in runs], dtype=float)
        pooled = np.concatenate([np.asarray(run["durations_ns"], dtype=float) for run in runs])
        median_low, median_high = bootstrap_ci(medians)
        p95_low, p95_high = bootstrap_ci(p95s)
        summary = {
            "recovery_samples": len(pooled),
            "min_samples_per_run": min(len(run["durations_ns"]) for run in runs),
            "median_run_median_ns": float(np.median(medians)),
            "median_ci95_low_ns": median_low,
            "median_ci95_high_ns": median_high,
            "median_run_p95_ns": float(np.median(p95s)),
            "p95_ci95_low_ns": p95_low,
            "p95_ci95_high_ns": p95_high,
            "pooled_p50_ns": float(np.percentile(pooled, 50)),
            "pooled_p99_ns": float(np.percentile(pooled, 99)),
            "pooled_max_ns": float(pooled.max()),
        }
    return pd.DataFrame(
        [
            {
                "condition": "panic-recovery",
                "N_runs": len(admitted),
                "runs_stopped_early": len(admitted) - len(runs),
                **summary,
                "units": "nanoseconds, samples",
                "estimator": "median over the completed runs of each run's median and p95 recovery, with bootstrap 95% CIs over runs; pooled percentiles are descriptive because samples within a run are not independent",
                "threshold": "none; descriptive recovery time",
                "claim_boundary": "trap to running for the declared panic stimulus on this host",
                "thesis_evidence": canonical,
            }
        ]
    )


DEPTH_CONDITIONS = ("depth-1", "depth-3", "depth-5", "depth-10")


def depth_tables(
    records: list[dict], value: str, *, units: str, canonical: bool = True
) -> tuple[pd.DataFrame, pd.DataFrame]:
    """Per-depth medians of one run-level ``value`` and its fitted per-depth slope.

    Returns (one row per depth, one row with the slope). The slope describes
    the trend across depths 1, 3, 5 and 10; it is not a decision rule.
    """
    grouped = _group_runs(records, DEPTH_CONDITIONS, canonical=canonical)
    rows = []
    for condition, runs in grouped.items():
        if not runs:
            continue
        values = np.asarray([run[value] for run in runs], dtype=float)
        low, high = bootstrap_ci(values)
        rows.append(
            {
                "condition": condition,
                "depth": int(condition.removeprefix("depth-")),
                "N_runs": len(runs),
                f"median_{value}": float(np.median(values)),
                "ci95_low": low,
                "ci95_high": high,
                "units": units,
                "estimator": "median run value with bootstrap 95% CI",
                "thesis_evidence": canonical,
            }
        )
    per_depth = pd.DataFrame(rows)
    if len(rows) < 2:
        return per_depth, pd.DataFrame()
    depths = [int(condition.removeprefix("depth-")) for condition, runs in grouped.items() for _ in runs]
    values = [run[value] for runs in grouped.values() for run in runs]
    slope, low, high, intercept, r_squared = stratified_slope(np.asarray(depths), np.asarray(values))
    fit = pd.DataFrame(
        [
            {
                "value": value,
                "depths": ", ".join(str(depth) for depth in per_depth.depth),
                "N_runs": len(values),
                "slope_per_depth": slope,
                "slope_ci95_low": low,
                "slope_ci95_high": high,
                "intercept": intercept,
                "r_squared": r_squared,
                "units": f"{units} per added transform",
                "estimator": "OLS slope over run-level values with a bootstrap 95% CI resampling runs within each depth",
                "claim_boundary": "describes the trend over the tested depths; not a decision rule and not an extrapolation",
                "thesis_evidence": canonical,
            }
        ]
    )
    return per_depth, fit


PAYLOAD_CONDITIONS = {"120b": 120, "1kb": 1_024, "10kb": 10_240, "100kb": 102_400}
NATIVE_PAYLOAD_PREFIX = "native-"


def payload_table(records: list[dict], *, canonical: bool = True) -> pd.DataFrame:
    """Wasm boundary cost per E-Perf-4 payload size: WAFER minus native service time.

    Each record is one run's ``service-percentiles.json`` values as
    ``service_p50_ns``, ``service_p95_ns`` and ``service_p99_ns`` plus its
    ``sequence.csv`` counts. A WAFER run is paired with the native run of the
    same payload size and run index, which ran in the same randomised block.
    """
    conditions = (
        *PAYLOAD_CONDITIONS,
        *(f"{NATIVE_PAYLOAD_PREFIX}{size}" for size in PAYLOAD_CONDITIONS),
    )
    grouped = _group_runs(records, conditions, canonical=canonical)
    reference = declared_thresholds()["e-perf-4-boundary-p50"]
    rows = []
    for size, payload_bytes in PAYLOAD_CONDITIONS.items():
        wafer_runs = {int(run["run_index"]): run for run in grouped[size]}
        native_runs = {
            int(run["run_index"]): run for run in grouped[f"{NATIVE_PAYLOAD_PREFIX}{size}"]
        }
        pairs = sorted(wafer_runs.keys() & native_runs.keys())
        if not pairs:
            continue
        wafer = [wafer_runs[index] for index in pairs]
        native = [native_runs[index] for index in pairs]
        service = {
            arm: {
                name: np.asarray([run[f"service_{name}_ns"] for run in runs], dtype=float)
                for name in ("p50", "p95", "p99")
            }
            for arm, runs in (("wafer", wafer), ("native", native))
        }
        row = {"condition": size, "payload_bytes": payload_bytes, "N_pairs": len(pairs)}
        for arm in ("wafer", "native"):
            low, high = bootstrap_ci(service[arm]["p50"])
            row.update(
                {
                    f"{arm}_median_service_p50_ns": float(np.median(service[arm]["p50"])),
                    f"{arm}_service_p50_ci95_low_ns": low,
                    f"{arm}_service_p50_ci95_high_ns": high,
                }
            )
        boundary_p50 = service["wafer"]["p50"] - service["native"]["p50"]
        for name in ("p50", "p95", "p99"):
            difference = service["wafer"][name] - service["native"][name]
            low, high = bootstrap_ci(difference)
            row.update(
                {
                    f"boundary_{name}_ns": float(np.median(difference)),
                    f"boundary_{name}_ci95_low_ns": low,
                    f"boundary_{name}_ci95_high_ns": high,
                }
            )
        delta, magnitude = cliffs_delta(service["wafer"]["p50"], service["native"]["p50"])
        delta_low, delta_high = cliffs_delta_ci(service["wafer"]["p50"], service["native"]["p50"])
        row.update(
            {
                "cliffs_delta_vs_native": delta,
                "cliffs_delta_ci95_low": delta_low,
                "cliffs_delta_ci95_high": delta_high,
                "effect_magnitude": magnitude,
                **bound_verdict(
                    "per_hop_reference",
                    reference,
                    bootstrap_ci(boundary_p50, ci=reference.interval),
                    estimate=float(np.median(boundary_p50)),
                ),
            }
        )
        for arm, runs in (("wafer", wafer), ("native", native)):
            expected = [int(run["total_expected"]) for run in runs]
            lost = [max(0, int(run["total_expected"]) - int(run["received_unique"])) for run in runs]
            loss_low, loss_high = pooled_ratio_ci(lost, expected)
            row.update(
                {
                    f"{arm}_pooled_loss": sum(lost) / sum(expected),
                    f"{arm}_pooled_loss_ci95_low": loss_low,
                    f"{arm}_pooled_loss_ci95_high": loss_high,
                    f"{arm}_duplicates": sum(int(run["duplicates"]) for run in runs),
                }
            )
        row.update(
            {
                "units": "bytes, nanoseconds, fraction, messages",
                "estimator": "median over run pairs of the WAFER-minus-native service-time p50, p95 and p99, pairing runs by index within the randomised block, with a bootstrap 95% CI over pairs; per-arm median service p50 with bootstrap 95% CIs; Cliff's delta of WAFER against native service p50 with bootstrap 95% CI; pooled loss per arm with a run-resampling bootstrap 95% CI; reference verdict from one-sided 95% bounds over pairs",
                "threshold": (
                    "one-sided 95% upper bound of the median WAFER-minus-native service p50 "
                    f"< {reference.value / 1_000:g} microseconds per hop (a reference, not a pass criterion)"
                ),
                "claim_boundary": "in-process path only: bench-source, one pass-through transform, bench-sink; the MQTT adapters keep rumqttc's 10 KiB packet limit and are not measured. The difference covers the whole Wasm stage, including metering and copies into and out of guest memory",
                "thesis_evidence": canonical,
            }
        )
        rows.append(row)
    return pd.DataFrame(rows)


def validation_gate_table(records: list[dict], *, canonical: bool = True) -> pd.DataFrame:
    """E-Val-1: how many runs recovered the injected 50 ms delay inside the runner's p99 band."""
    runs = _group_runs(records, ("delay-50ms",), canonical=canonical)["delay-50ms"]
    if not runs:
        return pd.DataFrame()
    rules = declared_thresholds()
    low_bound, high_bound = rules["e-val-1-p99-low"], rules["e-val-1-p99-high"]
    p50 = np.asarray([run["p50_ns"] for run in runs], dtype=float)
    p99 = np.asarray([run["p99_ns"] for run in runs], dtype=float)
    inside = sum(low_bound.holds(value) and high_bound.holds(value) for value in p99)
    p50_low, p50_high = bootstrap_ci(p50)
    p99_low, p99_high = bootstrap_ci(p99)
    return pd.DataFrame(
        [
            {
                "condition": "delay-50ms",
                "N_runs": len(runs),
                "runs_inside_band": inside,
                "median_p50_ns": float(np.median(p50)),
                "p50_ci95_low_ns": p50_low,
                "p50_ci95_high_ns": p50_high,
                "median_p99_ns": float(np.median(p99)),
                "p99_ci95_low_ns": p99_low,
                "p99_ci95_high_ns": p99_high,
                "gate_passed": inside == len(runs),
                "units": "nanoseconds, runs",
                "estimator": "count of runs whose p99 lies in the band; median run p50 and p99 with bootstrap 95% CIs",
                "threshold": (
                    f"every run's p99 between {low_bound.value / 1_000_000:g} ms and "
                    f"{high_bound.value:,.0f} ns (the HdrHistogram bucket holding 55 ms)"
                ),
                "claim_boundary": "the harness recovers a known 50 ms delay; it gates the other measurements and is not a WAFER result",
                "thesis_evidence": canonical,
            }
        ]
    )


def density_table(rows: list[dict], floor: dict, *, canonical: bool = True) -> pd.DataFrame:
    """E-Density-1: release Wasm component size beside the measured FROM scratch container floor."""
    floor_bytes = floor.get("image_bytes")
    if floor.get("base") != "scratch" or type(floor_bytes) is not int or floor_bytes <= 0:
        raise ValueError("the container floor needs a measured FROM scratch image size")
    table = pd.DataFrame(
        [{"plugin": row["plugin"], "wasm_bytes": int(row["wasm_bytes"])} for row in rows]
    )
    if table.empty:
        return table
    if (table.wasm_bytes <= 0).any() or table.plugin.duplicated().any():
        raise ValueError("binary sizes need one positive size per plugin")
    return table.assign(
        container_floor_bytes=floor_bytes,
        floor_to_wasm_ratio=floor_bytes / table.wasm_bytes,
        units="bytes",
        estimator="stat size of each release component; container floor is the measured uncompressed size of one FROM scratch image",
        claim_boundary="one FROM scratch image of a statically linked Rust stdin-to-stdout pass-through, not an image per plugin; the WAFER runtime and the container engine are outside both sizes",
        thesis_evidence=canonical,
    )


STARTUP_TIERS = ("small", "medium", "large")
STARTUP_PHASES = ("process_config", "component_load_compile", "instantiation", "pipeline_setup", "first_process")


def startup_table(records: list[dict], *, canonical: bool = True) -> pd.DataFrame:
    """Startup phases per tier and page-cache state, with the paired cold minus warm difference.

    Each record is one run's ``startup.json`` with its ``condition`` and ``run_index``.
    """
    conditions = tuple(f"{tier}-{state}" for tier in STARTUP_TIERS for state in ("cold", "warm"))
    grouped = _group_runs(records, conditions, canonical=canonical)
    totals = {
        condition: np.asarray([run["total_wall_duration_ns"] for run in runs], dtype=float)
        for condition, runs in grouped.items()
    }
    rows = []
    for condition, runs in grouped.items():
        if not runs:
            continue
        tier, state = condition.rsplit("-", 1)
        low, high = bootstrap_ci(totals[condition])
        phases = {
            phase: np.asarray([run["phases_ns"][phase] for run in runs], dtype=float)
            for phase in STARTUP_PHASES
        }
        unmeasured = totals[condition] - sum(phases.values())
        row = {
            "condition": condition,
            "tier": tier,
            "cache_state": state,
            "N_runs": len(runs),
            "median_total_ns": float(np.median(totals[condition])),
            "total_ci95_low_ns": low,
            "total_ci95_high_ns": high,
            **{f"median_{phase}_ns": float(np.median(values)) for phase, values in phases.items()},
            "median_unmeasured_ns": float(np.median(unmeasured)),
            "compiled_cache_hits": sum(bool(run["compiled_component_cache"]["hit"]) for run in runs),
            **dict.fromkeys(("N_pairs", "median_cold_minus_warm_ns", "difference_ci95_low_ns", "difference_ci95_high_ns", "hodges_lehmann_shift_ns", "shift_ci95_low_ns", "shift_ci95_high_ns", "cliffs_delta_vs_warm", "effect_magnitude")),
        }
        warm_by_run = {run["run_index"]: run["total_wall_duration_ns"] for run in grouped[f"{tier}-warm"]}
        pairs = [(run["total_wall_duration_ns"], warm_by_run[run["run_index"]]) for run in runs if run["run_index"] in warm_by_run]
        if state == "cold" and pairs:
            cold, warm = (np.asarray(side, dtype=float) for side in zip(*pairs))
            differences = cold - warm
            difference_low, difference_high = bootstrap_ci(differences)
            shift, shift_low, shift_high = hodges_lehmann(differences)
            delta, magnitude = cliffs_delta(cold, warm)
            row.update(
                {
                    "N_pairs": len(pairs),
                    "median_cold_minus_warm_ns": float(np.median(differences)),
                    "difference_ci95_low_ns": difference_low,
                    "difference_ci95_high_ns": difference_high,
                    "hodges_lehmann_shift_ns": shift,
                    "shift_ci95_low_ns": shift_low,
                    "shift_ci95_high_ns": shift_high,
                    "cliffs_delta_vs_warm": delta,
                    "effect_magnitude": magnitude,
                }
            )
        row.update(
            {
                "units": "nanoseconds, runs",
                "estimator": "median run total with bootstrap 95% CI; phase medians are descriptive and need not sum to the median total; cold minus warm paired by run index, because each warm start follows its cold start: median paired difference with bootstrap 95% CI and Hodges-Lehmann shift with bootstrap 95% CI; Cliff's delta is descriptive",
                "claim_boundary": "Linux page-cache state only; the compiled-component cache is disabled, so no compiled-cache claim",
                "thesis_evidence": canonical,
            }
        )
        rows.append(row)
    return pd.DataFrame(rows)


def _candidate_scaling_table(
    summary: dict,
    *,
    experiment: str,
    batch_class: str,
    sample_unit: str,
    values: tuple[tuple[str, int], ...],
    value_field: str,
    no_pool_with: list[str],
    require_rss: bool,
) -> pd.DataFrame:
    if {
        "schema_version": summary.get("schema_version"),
        "experiment": summary.get("experiment"),
        "evidence_class": summary.get("evidence_class"),
        "thesis_evidence": summary.get("thesis_evidence"),
        "n30_admitted": summary.get("n30_admitted"),
        "sample_unit": summary.get("sample_unit"),
        "required_runs_per_condition": summary.get("required_runs_per_condition"),
        "complete": summary.get("complete"),
        "no_pool_with": summary.get("no_pool_with"),
    } != {
        "schema_version": 1,
        "experiment": experiment,
        "evidence_class": "candidate-supplementary",
        "thesis_evidence": False,
        "n30_admitted": False,
        "sample_unit": sample_unit,
        "required_runs_per_condition": 5,
        "complete": True,
        "no_pool_with": no_pool_with,
    }:
        raise ValueError(f"malformed {experiment} candidate summary")

    expected_values = dict(values)
    expected_keys = {
        (condition, run_index)
        for condition in expected_values
        for run_index in range(1, 6)
    }
    observed_keys: set[tuple[str, int]] = set()
    source_shas: set[str] = set()
    rows = []
    for record in summary.get("records", []):
        if (
            record.get("experiment") != experiment
            or record.get("batch_class") != batch_class
            or record.get("evidence_class") != "candidate-supplementary"
            or record.get("thesis_evidence") is not False
            or record.get("n30_admitted") is not False
            or record.get("sample_unit") != sample_unit
        ):
            raise ValueError(f"{experiment} record has invalid candidate identity")
        condition = record.get("condition")
        run_index = record.get("run_index")
        if condition not in expected_values or run_index not in range(1, 6):
            raise ValueError(f"{experiment} record is outside the frozen N=5 grid")
        if record.get(value_field) != expected_values[condition]:
            raise ValueError(f"{experiment} record condition value differs from its label")
        key = (condition, run_index)
        if key in observed_keys:
            raise ValueError(f"{experiment} contains duplicate run identity {key}")
        observed_keys.add(key)
        source_sha = record.get("source_git_sha")
        if not isinstance(source_sha, str) or len(source_sha) != 40 or record.get("source_dirty") is not False:
            raise ValueError(f"{experiment} record has invalid source provenance")
        source_shas.add(source_sha)
        latency = record.get("latency_ns")
        if not isinstance(latency, dict) or any(
            type(latency.get(field)) is not int or latency[field] < 0
            for field in ("p50", "p95", "p99")
        ):
            raise ValueError(f"{experiment} record has invalid latency evidence")
        row = {
            "condition": condition,
            "run_index": run_index,
            value_field: expected_values[condition],
            "p50_ns": latency["p50"],
            "p95_ns": latency["p95"],
            "p99_ns": latency["p99"],
            "source_git_sha": source_sha,
            "thesis_evidence": False,
            "n30_admitted": False,
        }
        if value_field == "payload_bytes":
            expected_hash = hashlib.sha256(b"B" * expected_values[condition]).hexdigest()
            if record.get("payload_sha256") != expected_hash:
                raise ValueError(f"{experiment} record has invalid payload hash")
            row["payload_sha256"] = expected_hash
        if require_rss:
            if (
                record.get("node_count") != expected_values[condition] + 2
                or record.get("transform_count") != expected_values[condition]
                or record.get("edge_count") != expected_values[condition] + 1
                or record.get("identical_transform_behavior") is not True
                or record.get("effective_metering_mode") != "fuel-and-epoch"
            ):
                raise ValueError(f"{experiment} record has invalid topology evidence")
            peak_rss = record.get("peak_rss_bytes")
            if type(peak_rss) is not int or peak_rss <= 0:
                raise ValueError(f"{experiment} record has invalid RSS evidence")
            row["peak_rss_bytes"] = peak_rss
        rows.append(row)
    if observed_keys != expected_keys:
        raise ValueError(f"{experiment} requires five independent runs per condition")
    if len(source_shas) != 1:
        raise ValueError(f"{experiment} summary mixes source revisions")
    return pd.DataFrame(rows).sort_values([value_field, "run_index"]).reset_index(drop=True)


def candidate_payload_table(summary: dict) -> pd.DataFrame:
    return _candidate_scaling_table(
        summary,
        experiment="e-perf-payload-refinement",
        batch_class="candidate-payload-refinement",
        sample_unit="independent host run at one payload size",
        values=(
            ("120b", 120),
            ("1kb", 1_024),
            ("8kb", 8_192),
            ("10kb", 10_240),
            ("16kb", 16_384),
            ("32kb", 32_768),
            ("64kb", 65_536),
            ("100kb", 102_400),
            ("128kb", 131_072),
            ("256kb", 262_144),
        ),
        value_field="payload_bytes",
        no_pool_with=["e-perf-4", "prior diagnostic rehearsals"],
        require_rss=False,
    )


def candidate_depth_table(summary: dict) -> pd.DataFrame:
    return _candidate_scaling_table(
        summary,
        experiment="e-perf-depth-extension",
        batch_class="candidate-depth-extension",
        sample_unit="independent host run at one pipeline depth",
        values=tuple((f"depth-{depth}", depth) for depth in (1, 3, 5, 10, 20, 50)),
        value_field="depth",
        no_pool_with=[
            "e-perf-3",
            "e-perf-6",
            "e-perf-8",
            "prior diagnostic rehearsals",
        ],
        require_rss=True,
    )


def candidate_swap_tables(summary: dict) -> tuple[pd.DataFrame, pd.DataFrame]:
    experiment = summary.get("experiment")
    specifications = {
        "e-swap-independent-sessions": {
            "batch_class": "candidate-independent-swap",
            "condition": "steady",
            "nested_unit": "swap event within run",
            "no_pool_with": [
                "e-swap-1",
                "e-swap-2",
                "e-swap-6",
                "prior diagnostic rehearsals",
            ],
            "metrics": (
                "compile_ns",
                "instantiate_ns",
                "signal_ns",
                "replacement_adopted_ns",
                "first_post_replacement_local_outcome_ns",
                "http_total_ns",
                "sink_observed_output_gap_ns",
            ),
        },
        "e-swap-rollback-sessions": {
            "batch_class": "candidate-rollback-session",
            "condition": "process-trap-rollback",
            "nested_unit": "rollback event within run",
            "no_pool_with": ["e-swap-5", "prior diagnostic rehearsals"],
            "metrics": (
                "compile_ns",
                "instantiate_ns",
                "signal_ns",
                "rollback_ns",
                "http_total_ns",
            ),
        },
    }
    if experiment not in specifications:
        raise ValueError("unexpected candidate swap experiment")
    specification = specifications[experiment]
    expected_identity = {
        "schema_version": 1,
        "experiment": experiment,
        "evidence_class": "candidate-supplementary",
        "thesis_evidence": False,
        "n30_admitted": False,
        "sample_unit": "independent host run",
        "nested_unit": specification["nested_unit"],
        "required_runs": 5,
        "events_per_run": 50,
        "event_classes": ["first-use-aot", "cached"],
        "complete": True,
        "no_pool_with": specification["no_pool_with"],
    }
    if any(summary.get(key) != value for key, value in expected_identity.items()):
        raise ValueError(f"malformed {experiment} candidate summary")

    expected_runs = set(range(1, 6))
    observed_runs: set[int] = set()
    source_shas: set[str] = set()
    event_rows = []
    run_rows = []
    for record in summary.get("records", []):
        run_index = record.get("run_index")
        if (
            record.get("experiment") != experiment
            or record.get("batch_class") != specification["batch_class"]
            or record.get("condition") != specification["condition"]
            or record.get("evidence_class") != "candidate-supplementary"
            or record.get("thesis_evidence") is not False
            or record.get("n30_admitted") is not False
            or record.get("sample_unit") != "independent host run"
            or record.get("nested_unit") != specification["nested_unit"]
            or record.get("duration_unit") != "ns"
            or record.get("event_classes") != ["first-use-aot", "cached"]
            or record.get("sample_count") != 50
            or record.get("shared_from") is not None
            or record.get("no_pool_with") != specification["no_pool_with"]
            or run_index not in expected_runs
            or run_index in observed_runs
        ):
            raise ValueError(f"{experiment} record has invalid independent-run identity")
        observed_runs.add(run_index)
        source_sha = record.get("source_git_sha")
        if not isinstance(source_sha, str) or len(source_sha) != 40 or record.get("source_dirty") is not False:
            raise ValueError(f"{experiment} record has invalid source provenance")
        source_shas.add(source_sha)
        sequence = record.get("sequence")
        if (
            not isinstance(sequence, dict)
            or sequence.get("expected") != sequence.get("received")
            or sequence.get("gaps") != 0
            or sequence.get("duplicates") != 0
        ):
            raise ValueError(f"{experiment} record is not lossless")
        events = record.get("events")
        if not isinstance(events, list) or len(events) != 50:
            raise ValueError(f"{experiment} record requires exactly 50 events")
        if experiment == "e-swap-rollback-sessions" and (
            record.get("attempts") != 50
            or record.get("rolled_back") != 50
            or record.get("all_rolled_back") is not True
        ):
            raise ValueError("rollback candidate requires fifty successful rollbacks")
        class_rows = {"first-use-aot": [], "cached": []}
        for expected_index, event in enumerate(events):
            expected_class = "first-use-aot" if expected_index == 0 else "cached"
            expected_plugin = (
                "wafer_pass_through_v2_panics.wasm"
                if experiment == "e-swap-rollback-sessions"
                else "wafer_pass_through_v2.wasm"
                if expected_index % 2 == 0
                else "wafer_pass_through_v1.wasm"
            )
            if (
                event.get("event_index") != expected_index
                or event.get("event_class") != expected_class
                or event.get("plugin") != expected_plugin
            ):
                raise ValueError(f"{experiment} event classes, indices, or plugins are mixed")
            if any(type(event.get(metric)) is not int or event[metric] < 0 for metric in specification["metrics"]):
                raise ValueError(f"{experiment} event contains invalid duration evidence")
            row = {
                "run_index": run_index,
                "event_index": expected_index,
                "event_class": expected_class,
                "source_git_sha": source_sha,
                "thesis_evidence": False,
                **{metric: event[metric] for metric in specification["metrics"]},
            }
            event_rows.append(row)
            class_rows[expected_class].append(row)
        for event_class, rows in class_rows.items():
            run_rows.append(
                {
                    "run_index": run_index,
                    "event_class": event_class,
                    "event_count": len(rows),
                    **{
                        f"median_{metric}": statistics.median(row[metric] for row in rows)
                        for metric in specification["metrics"]
                    },
                }
            )
    if observed_runs != expected_runs or len(source_shas) != 1:
        raise ValueError(f"{experiment} requires five clean runs from one source revision")
    return (
        pd.DataFrame(event_rows).sort_values(["run_index", "event_index"]).reset_index(drop=True),
        pd.DataFrame(run_rows).sort_values(["event_class", "run_index"]).reset_index(drop=True),
    )


def ekuiper_profile_tables(summary: dict) -> tuple[pd.DataFrame, pd.DataFrame]:
    identity = {
        "schema_version": 1,
        "experiment": "e-compare-ekuiper-profile",
        "batch_class": "diagnostic-ekuiper-profile",
        "evidence_class": "diagnostic",
        "thesis_evidence": False,
        "n30_admitted": False,
        "sample_unit": "independent host run at one rate and profiler state",
        "required_runs_per_cell": 5,
        "rates_msg_s": [1_000, 4_000, 8_000],
        "profiler_states": ["profiled", "unprofiled-control"],
        "complete": True,
        "claim_boundary": "diagnostic-association-only-not-gc-causality",
        "no_pool_with": ["e-perf-1", "e-perf-10", "prior diagnostic rehearsals"],
    }
    if any(summary.get(field) != value for field, value in identity.items()):
        raise ValueError("eKuiper profile summary diagnostic identity is invalid")
    records = summary.get("records")
    if not isinstance(records, list):
        raise ValueError("eKuiper profile summary records are invalid")
    expected_keys = {
        (rate, state, run_index)
        for rate in (1_000, 4_000, 8_000)
        for state in ("profiled", "unprofiled-control")
        for run_index in range(1, 6)
    }
    observed_keys = set()
    source_shas = set()
    rows = []
    for record in records:
        key = (
            record.get("rate_msg_s"),
            record.get("profiler_state"),
            record.get("run_index"),
        )
        if key in observed_keys:
            raise ValueError("eKuiper profile summary duplicates a matched run")
        observed_keys.add(key)
        condition = f"rate-{key[0]:05d}/{key[1]}" if key[0] and key[1] else ""
        if (
            record.get("schema_version") != 1
            or record.get("experiment") != "e-compare-ekuiper-profile"
            or record.get("evidence_class") != "diagnostic"
            or record.get("thesis_evidence") is not False
            or record.get("n30_admitted") is not False
            or record.get("sample_unit")
            != "independent host run at one rate and profiler state"
            or record.get("condition") != condition
            or record.get("shared_from") is not None
            or not isinstance(record.get("measurement_source_leaf"), str)
            or record.get("claim_boundary")
            != "diagnostic-association-only-not-gc-causality"
            or record.get("no_pool_with") != identity["no_pool_with"]
        ):
            raise ValueError("eKuiper profile record crosses the diagnostic boundary")
        gc_runtime = record.get("gc_runtime_metrics", {})
        if gc_runtime.get("status") == "available":
            if key[1] != "profiled" or any(
                type(gc_runtime.get(field)) is not int or gc_runtime[field] < 0
                for field in ("cycle_count", "missing_cycle_count", "stw_pause_total_ns")
            ):
                raise ValueError("eKuiper profile GC trace evidence is invalid")
        elif (
            gc_runtime.get("status") != "unavailable"
            or not gc_runtime.get("reason")
            or (key[1] == "profiled")
            == (gc_runtime["reason"] == "gctrace-disabled-by-design")
        ):
            raise ValueError("eKuiper profile GC trace availability is invalid")
        interval = record.get("interval_alignment", {})
        if (
            interval.get("clock") != "unix-epoch"
            or not 1 <= int(interval.get("row_count", 0)) <= int(interval.get("maximum_rows", 0))
            or int(interval.get("measurement_end_ns", 0))
            - int(interval.get("measurement_start_ns", 0))
            != 60_000_000_000
            or not isinstance(interval.get("path"), str)
            or not isinstance(interval.get("sha256"), str)
            or len(interval["sha256"]) != 64
        ):
            raise ValueError("eKuiper profile interval alignment is invalid")
        process = record.get("process_metrics", {})
        if process.get("status") == "available":
            if (
                key[1] != "profiled"
                or not 2 <= int(process.get("row_count", 0)) <= 62
                or process.get("maximum_rows") != 62
            ):
                raise ValueError("eKuiper profile process evidence is invalid")
        elif process.get("status") != "unavailable" or not process.get("reason"):
            raise ValueError("eKuiper profile process availability is invalid")
        latency = record.get("latency_ns", {})
        if any(type(latency.get(field)) is not int or latency[field] < 0 for field in ("sample_count", "p50", "p95", "p99")):
            raise ValueError("eKuiper profile latency evidence is invalid")
        source_sha = record.get("source_git_sha")
        if (
            not isinstance(source_sha, str)
            or len(source_sha) != 40
            or record.get("source_dirty") is not False
        ):
            raise ValueError("eKuiper profile source provenance is invalid")
        source_shas.add(source_sha)
        overhead = record.get("profiler_overhead", {})
        paired_state = (
            "unprofiled-control" if key[1] == "profiled" else "profiled"
        )
        if (
            overhead.get("experiment") != "e-compare-ekuiper-profile"
            or overhead.get("condition") != condition
            or overhead.get("run_index") != key[2]
            or overhead.get("rate_msg_s") != key[0]
            or overhead.get("profiler_state") != key[1]
            or overhead.get("paired_condition")
            != f"rate-{key[0]:05d}/{paired_state}"
            or overhead.get("pair_key") != f"rate-{key[0]:05d}/run-{key[2]:02d}"
            or overhead.get("overhead_estimator")
            != "paired-run-level-profiled-minus-unprofiled-control"
            or overhead.get("claim_boundary") != identity["claim_boundary"]
        ):
            raise ValueError("eKuiper profile pairing evidence is invalid")
        rows.append(
            {
                "rate_msg_s": key[0],
                "profiler_state": key[1],
                "run_index": key[2],
                "sample_count": latency["sample_count"],
                "p50_ns": latency["p50"],
                "p95_ns": latency["p95"],
                "p99_ns": latency["p99"],
                "process_status": process["status"],
                "cpu_percent": process.get("cpu_percent"),
                "max_rss_bytes": process.get("max_rss_bytes"),
                "gc_status": gc_runtime["status"],
                "gc_cycle_count": gc_runtime.get("cycle_count"),
                "gc_stw_pause_total_ns": gc_runtime.get("stw_pause_total_ns"),
                "gc_stw_pause_max_ns": gc_runtime.get("stw_pause_max_ns"),
                "measurement_source_leaf": record["measurement_source_leaf"],
                "interpretation": identity["claim_boundary"],
            }
        )
    if observed_keys != expected_keys:
        raise ValueError("eKuiper profile summary does not contain the exact matched run grid")
    if len(source_shas) != 1:
        raise ValueError("eKuiper profile summary mixes source revisions")
    runs = pd.DataFrame(rows).sort_values(
        ["rate_msg_s", "run_index", "profiler_state"]
    ).reset_index(drop=True)
    pairs = []
    for (rate, run_index), group in runs.groupby(["rate_msg_s", "run_index"]):
        by_state = group.set_index("profiler_state")
        profiled = by_state.loc["profiled"]
        control = by_state.loc["unprofiled-control"]
        pairs.append(
            {
                "rate_msg_s": rate,
                "run_index": run_index,
                "p95_overhead_ns": profiled.p95_ns - control.p95_ns,
                "p99_overhead_ns": profiled.p99_ns - control.p99_ns,
                "p95_ratio": profiled.p95_ns / control.p95_ns,
                "p99_ratio": profiled.p99_ns / control.p99_ns,
                "interpretation": identity["claim_boundary"],
            }
        )
    return runs, pd.DataFrame(pairs).sort_values(["rate_msg_s", "run_index"]).reset_index(drop=True)


def candidate_capacity_table(summary: dict) -> pd.DataFrame:
    grids = {
        "mqtt-loopback": [
            *range(4_000, 16_000, 1_000),
            15_250,
            15_500,
            15_750,
            16_000,
        ],
        "native": list(range(8_000, 16_000, 1_000)),
        "wafer": list(range(8_000, 16_000, 1_000)),
        "ekuiper": list(range(4_000, 9_000, 1_000)),
    }
    criteria = {
        "loss_aggregation": "sum(total_undelivered) / sum(intended)",
        "max_pooled_loss": 0.01,
        "achieved_aggregation": "mean(run achieved_rate / intended_rate)",
        "min_mean_achieved_ratio": 0.99,
        "duplicates_allowed": 0,
        "support_path_censoring": "mqtt-loopback",
    }
    if (
        summary.get("schema_version") != 1
        or summary.get("experiment") != "e-perf-capacity-knee"
        or summary.get("evidence_class") != "candidate-supplementary"
        or summary.get("thesis_evidence") is not False
        or summary.get("n30_admitted") is not False
        or summary.get("sample_unit")
        != "independent host run at one system and offered rate"
        or summary.get("required_runs_per_rate") != 5
    ):
        raise ValueError("malformed candidate capacity summary")
    if summary.get("criteria") != criteria:
        raise ValueError("candidate capacity criteria differ from the frozen estimator")
    systems = summary.get("systems")
    if not isinstance(systems, dict) or set(systems) != set(grids):
        raise ValueError("candidate capacity summary requires all four systems")

    mqtt_rates = systems["mqtt-loopback"].get("rates", [])
    first_support_bad = next(
        (
            int(rate["rate_msg_s"])
            for rate in mqtt_rates
            if rate.get("classification") == "bad"
        ),
        None,
    )
    rows = []
    for system, grid in grids.items():
        result = systems[system]
        rates = result.get("rates")
        if not result.get("complete") or not isinstance(rates, list):
            raise ValueError(f"{system} candidate capacity summary is incomplete")
        if [rate.get("rate_msg_s") for rate in rates] != grid:
            raise ValueError(f"{system} candidate capacity rates differ from the frozen grid")
        expected_support = {
            "from_rate_msg_s": first_support_bad,
            "highest_support_uncensored_rate_msg_s": max(
                (rate for rate in grid if first_support_bad is None or rate < first_support_bad),
                default=None,
            ),
        }
        if result.get("support_censoring") != expected_support:
            raise ValueError(f"{system} MQTT support-path censoring is invalid")
        for rate in rates:
            if rate.get("run_count") != 5:
                raise ValueError(f"{system} rate requires five independent runs")
            pooled_loss = float(rate["pooled_loss"])
            achieved_ratio = float(rate["mean_achieved_ratio"])
            duplicates = int(rate["duplicates"])
            raw = (
                "good"
                if pooled_loss <= 0.01
                and achieved_ratio >= 0.99
                and duplicates == 0
                else "bad"
            )
            expected = (
                "support-confounded"
                if system != "mqtt-loopback"
                and first_support_bad is not None
                and int(rate["rate_msg_s"]) >= first_support_bad
                else raw
            )
            if rate.get("classification") != expected:
                raise ValueError(
                    f"{system} rate {rate['rate_msg_s']} classification disagrees "
                    "with the frozen estimator or MQTT censoring"
                )
            rows.append(
                {
                    "system": system,
                    "offered_rate_msg_s": int(rate["rate_msg_s"]),
                    "N_runs": 5,
                    "pooled_loss": pooled_loss,
                    "mean_achieved_ratio": achieved_ratio,
                    "total_duplicates": duplicates,
                    "classification": expected,
                    "delivery_good": expected == "good",
                    "support_confounded": expected == "support-confounded",
                    "thesis_evidence": False,
                    "n30_admitted": False,
                }
            )
    return pd.DataFrame(rows)


CAPACITY_BASELINE_RATE = 1_000
CAPACITY_RUN_METRICS = ("achieved_rate_msg_s", "achieved_ratio", "loss", "p99_ns")


def _capacity_grid(summary: dict) -> list[int]:
    grid = summary.get("rate_points_msg_s")
    if (
        not isinstance(grid, list)
        or not grid
        or any(type(rate) is not int or rate <= 0 for rate in grid)
        or any(low >= high for low, high in zip(grid, grid[1:]))
    ):
        raise ValueError("capacity rate grid must be strictly increasing positive rates")
    return grid


def _capacity_cell_classes(summary: dict, systems: Iterable[str]) -> dict[str, list[str]]:
    grid = _capacity_grid(summary)
    results = summary.get("systems")
    if not isinstance(results, dict):
        raise ValueError("capacity population is malformed")
    required = summary.get("required_runs_per_rate")
    rules = declared_thresholds()
    loss_rule = rules["e-perf-10-cell-loss"]
    achieved_rule = rules["e-perf-10-cell-achieved-ratio"]
    duplicate_rule = rules["e-perf-10-cell-duplicates"]

    def classify(system: str, support_from: int | None) -> list[str]:
        result = results.get(system)
        rates = result.get("rates") if isinstance(result, dict) else None
        if not isinstance(rates, list) or not result.get("complete"):
            raise ValueError(f"{system}: incomplete system population")
        if [rate.get("rate_msg_s") for rate in rates] != grid:
            raise ValueError(f"{system}: system rates differ from tested grid")
        classes = []
        for rate in rates:
            run_count = rate.get("run_count")
            outcome_runs = rate.get("sut_outcome_runs", 0)
            if (
                type(run_count) is not int
                or type(outcome_runs) is not int
                or min(run_count, outcome_runs) < 0
                or run_count + outcome_runs != required
            ):
                raise ValueError(f"{system}: incomplete tested-rate cell")
            try:
                good = (
                    outcome_runs == 0
                    and loss_rule.holds(float(rate["pooled_loss"]))
                    and achieved_rule.holds(float(rate["mean_achieved_ratio"]))
                    and duplicate_rule.holds(int(rate["total_duplicates"]))
                )
            except (KeyError, TypeError, ValueError) as error:
                raise ValueError(f"{system}: malformed delivery counters") from error
            if support_from is not None and rate["rate_msg_s"] >= support_from:
                classes.append("support-confounded")
            else:
                classes.append("good" if good else "bad")
        return classes

    support = classify("mqtt-loopback", None)
    support_from = next(
        (rate for rate, value in zip(grid, support, strict=True) if value == "bad"), None
    )
    return {
        system: support if system == "mqtt-loopback" else classify(system, support_from)
        for system in systems
    }


def _ceiling_bounds(grid: list[int], classes: list[str]) -> dict:
    good = [index for index, value in enumerate(classes) if value == "good"]
    bad = [index for index, value in enumerate(classes) if value == "bad"]
    highest_good = max(good, default=-1)
    first_bad = min(bad, default=len(classes))
    sustained = [index for index in good if index < first_bad]
    above = [index for index in bad if index > highest_good]
    confounded = [
        rate
        for rate, value in zip(grid, classes, strict=True)
        if value == "support-confounded"
    ]
    return {
        "lower_bound_msg_s": grid[sustained[-1]] if sustained else 0,
        "upper_bound_msg_s": grid[above[0]] if above else math.inf,
        "non_monotonic": first_bad < highest_good,
        "support_confounded_rate_msg_s": confounded[0] if confounded else None,
    }


def capacity_competitive_decision(summary: dict) -> dict:
    grid = summary.get("rate_points_msg_s")
    rule = declared_thresholds()["e-perf-10-competitive-ratio"]
    common = {
        "threshold": rule.value,
        "beyond_grid_limitation": (
            f"tested-grid only; no claim beyond {max(grid)} msg/s"
            if isinstance(grid, list) and grid and all(type(rate) is int for rate in grid)
            else "tested grid is unavailable"
        ),
        "claim_boundary": (
            "co-located gateway envelope; each delivery ceiling is bracketed by "
            "tested rates and bounded by the MQTT support path"
        ),
    }
    try:
        classes = _capacity_cell_classes(summary, ("wafer", "ekuiper"))
    except ValueError as error:
        return {
            **common,
            "systems": {},
            "support_confounded_rate_msg_s": None,
            "ratio_lower_bound": None,
            "ratio_upper_bound": None,
            "branch": "invalid-population",
            "status": "PENDING",
            "reason": str(error),
        }
    bounds = {system: _ceiling_bounds(grid, value) for system, value in classes.items()}
    wafer = bounds["wafer"]
    ekuiper = bounds["ekuiper"]
    worst = wafer["lower_bound_msg_s"] / ekuiper["upper_bound_msg_s"]
    best = (
        math.inf
        if ekuiper["lower_bound_msg_s"] == 0
        else wafer["upper_bound_msg_s"] / ekuiper["lower_bound_msg_s"]
    )
    if rule.holds(worst):
        branch, status = "worst-case-pass", "PASS"
        reason = "WAFER lower bound divided by eKuiper upper bound meets the threshold"
    elif not rule.holds(best):
        branch, status = "best-case-fail", "FAIL"
        reason = "WAFER upper bound divided by eKuiper lower bound misses the threshold"
    else:
        branch, status = "straddles-threshold", "CENSORED"
        reason = "the tested grid allows ratios on both sides of the threshold"
    return {
        **common,
        "systems": bounds,
        "support_confounded_rate_msg_s": min(
            (
                value["support_confounded_rate_msg_s"]
                for value in bounds.values()
                if value["support_confounded_rate_msg_s"] is not None
            ),
            default=None,
        ),
        "ratio_lower_bound": worst,
        "ratio_upper_bound": best,
        "branch": branch,
        "status": status,
        "reason": reason,
    }


def _capacity_run_values(system: str, rate: dict) -> dict[str, np.ndarray]:
    run_count = rate["run_count"]
    if run_count == 0:
        return {metric: np.asarray([], dtype=float) for metric in CAPACITY_RUN_METRICS}
    run_summary = rate.get("run_summary")
    if not isinstance(run_summary, dict):
        raise TypeError(f"{system} rate summary is malformed")
    try:
        values = {
            metric: np.asarray(
                [float(value) for value in run_summary[metric]["values"]], dtype=float
            )
            for metric in CAPACITY_RUN_METRICS
        }
    except (KeyError, TypeError, ValueError) as error:
        raise ValueError(f"{system} rate summary is malformed") from error
    if any(len(samples) != run_count for samples in values.values()):
        raise ValueError(f"{system} rate summary needs {run_count} run values per metric")
    # Every admitted run at one rate offers the same intended count, so the
    # pooled loss equals the mean run loss.
    if not math.isclose(
        float(rate["pooled_loss"]), float(np.mean(values["loss"])), abs_tol=1e-12
    ) or not math.isclose(
        float(rate["mean_achieved_ratio"]),
        float(np.mean(values["achieved_ratio"])),
        abs_tol=1e-12,
    ):
        raise ValueError(
            f"{system} rate {rate['rate_msg_s']} pooled counters disagree with its run values"
        )
    return values


def _run_spread(metric: str, values: np.ndarray, interval: tuple[str, str]) -> dict:
    if not len(values):
        keys = (f"median_{metric}", *interval, *(f"{q}_{metric}" for q in ("min", "q1", "q3", "max")))
        return dict.fromkeys(keys)
    low, high = bootstrap_ci(values)
    return {
        f"median_{metric}": float(np.median(values)),
        interval[0]: float(low),
        interval[1]: float(high),
        f"min_{metric}": float(np.min(values)),
        f"q1_{metric}": float(np.quantile(values, 0.25)),
        f"q3_{metric}": float(np.quantile(values, 0.75)),
        f"max_{metric}": float(np.max(values)),
    }


def capacity_tables(summary: dict) -> tuple[pd.DataFrame, pd.DataFrame]:
    if (
        summary.get("schema_version") != 1
        or summary.get("experiment") != "e-perf-10"
        or summary.get("thesis_evidence") is not True
        or summary.get("sample_unit") != "run"
        or summary.get("required_runs_per_rate") != 30
    ):
        raise ValueError("malformed final capacity summary")
    grid = _capacity_grid(summary)
    if CAPACITY_BASELINE_RATE not in grid:
        raise ValueError("final capacity grid lacks the 1,000 msg/s baseline rate")
    systems = summary.get("systems")
    if not isinstance(systems, dict) or set(systems) != {
        "mqtt-loopback",
        "native",
        "wafer",
        "ekuiper",
    }:
        raise ValueError("final capacity summary requires all four systems")
    classes = _capacity_cell_classes(summary, systems)
    support_from = next(
        (
            rate
            for rate, value in zip(grid, classes["mqtt-loopback"], strict=True)
            if value == "bad"
        ),
        None,
    )
    run_values = {}
    for system, result in systems.items():
        for rate, classification in zip(result["rates"], classes[system], strict=True):
            if rate.get("classification") != classification:
                raise ValueError(
                    f"{system} rate {rate['rate_msg_s']} classification disagrees "
                    "with its delivery counters or MQTT censoring"
                )
        run_values[system] = [_capacity_run_values(system, rate) for rate in result["rates"]]
        support = result.get("support_censoring")
        if not isinstance(support, dict) or support.get("from_rate_msg_s") != support_from:
            raise ValueError(
                f"{system} MQTT support-path censoring disagrees with the loopback counters"
            )
    decision = capacity_competitive_decision(summary)
    rate_rows = []
    boundary_rows = []
    for system, result in systems.items():
        rates = result["rates"]
        values = run_values[system]
        baseline_p99 = values[grid.index(CAPACITY_BASELINE_RATE)]["p99_ns"]
        for rate, classification, runs in zip(rates, classes[system], values, strict=True):
            completed = len(runs["loss"]) > 0
            pooled_loss_ci = (
                pooled_ratio_ci(runs["loss"], np.ones(len(runs["loss"])))
                if completed
                else (None, None)
            )
            if completed and len(baseline_p99):
                _, normalized_low, normalized_high = median_shift_ci(
                    runs["p99_ns"], baseline_p99, relative=True
                )
                normalized = (
                    float(np.median(runs["p99_ns"]) / np.median(baseline_p99)),
                    1 + normalized_low,
                    1 + normalized_high,
                )
            else:
                normalized = (None, None, None)
            rate_rows.append(
                {
                    "system": system,
                    "offered_rate_msg_s": int(rate["rate_msg_s"]),
                    "N_runs": 30,
                    "sut_outcome_runs": rate.get("sut_outcome_runs", 0),
                    **_run_spread(
                        "achieved_rate_msg_s",
                        runs["achieved_rate_msg_s"],
                        ("achieved_ci95_low_msg_s", "achieved_ci95_high_msg_s"),
                    ),
                    **_run_spread(
                        "achieved_ratio",
                        runs["achieved_ratio"],
                        ("achieved_ratio_ci95_low", "achieved_ratio_ci95_high"),
                    ),
                    **_run_spread("loss", runs["loss"], ("loss_ci95_low", "loss_ci95_high")),
                    "pooled_loss": _number(rate["pooled_loss"] if completed else None),
                    "pooled_loss_ci95_low": _number(pooled_loss_ci[0]),
                    "pooled_loss_ci95_high": _number(pooled_loss_ci[1]),
                    "mean_achieved_ratio": _number(
                        rate["mean_achieved_ratio"] if completed else None
                    ),
                    "total_duplicates": int(rate["total_duplicates"]) if completed else None,
                    **_run_spread("p99_ns", runs["p99_ns"], ("p99_ci95_low_ns", "p99_ci95_high_ns")),
                    "median_normalized_p99": normalized[0],
                    "normalized_p99_ci95_low": normalized[1],
                    "normalized_p99_ci95_high": normalized[2],
                    "classification": classification,
                    "delivery_good": classification == "good",
                    "support_confounded": classification == "support-confounded",
                    "units": "messages/second, fraction, nanoseconds, messages",
                    "estimator": (
                        "run-level min/quartiles/max with bootstrap 95% CI of the median; "
                        "pooled loss with a run-resampling bootstrap 95% CI; mean achieved "
                        "ratio; normalized p99 CI resamples this rate and the 1,000 msg/s runs; "
                        "all over the runs that completed, and a run the system under test "
                        "failed makes its rate delivery-bad"
                    ),
                    "thesis_evidence": True,
                }
            )
        support = result["support_censoring"]
        bounds = _ceiling_bounds(grid, classes[system])
        boundary_rows.append(
            {
                "system": system,
                "delivery_ceiling_lower_bound_msg_s": bounds["lower_bound_msg_s"],
                "delivery_ceiling_upper_bound_msg_s": bounds["upper_bound_msg_s"],
                "delivery_ceiling_non_monotonic": bounds["non_monotonic"],
                "normalized_p99_knee_msg_s": result["normalized_p99_knee"][
                    "rate_msg_s"
                ],
                "normalized_p99_knee_censoring": result["normalized_p99_knee"][
                    "censoring"
                ],
                "mqtt_support_path_limitation": support["from_rate_msg_s"],
                "highest_support_uncensored_rate_msg_s": support[
                    "highest_support_uncensored_rate_msg_s"
                ],
                "wafer_lower_bound_msg_s": decision["systems"]["wafer"][
                    "lower_bound_msg_s"
                ],
                "wafer_upper_bound_msg_s": decision["systems"]["wafer"][
                    "upper_bound_msg_s"
                ],
                "ekuiper_lower_bound_msg_s": decision["systems"]["ekuiper"][
                    "lower_bound_msg_s"
                ],
                "ekuiper_upper_bound_msg_s": decision["systems"]["ekuiper"][
                    "upper_bound_msg_s"
                ],
                "competitive_ratio_lower_bound": decision["ratio_lower_bound"],
                "competitive_ratio_upper_bound": decision["ratio_upper_bound"],
                "competitive_threshold": decision["threshold"],
                "competitive_branch": decision["branch"],
                "competitive_status": decision["status"],
                "competitive_reason": decision["reason"],
                "support_confounded_rate_msg_s": decision[
                    "support_confounded_rate_msg_s"
                ],
                "beyond_grid_limitation": decision["beyond_grid_limitation"],
                "units": "messages/second",
                "claim_boundary": decision["claim_boundary"],
                "thesis_evidence": True,
            }
        )
    return pd.DataFrame(rate_rows), pd.DataFrame(boundary_rows)


def backpressure_table(records: list[dict], *, canonical: bool = True) -> pd.DataFrame:
    grouped = _require_runs(records, BACKPRESSURE_POLICIES)
    rows = []
    for policy, admitted in grouped.items():
        runs = [run for run in admitted if not _stopped_early(run)]
        for run in runs:
            validate_backpressure_result(run, policy)
        counts = [run["counts"] for run in runs]
        rows.append(
            {
                "policy": policy,
                "N_runs": len(admitted),
                "runs_stopped_early": len(admitted) - len(runs),
                "sut_outcome_runs": sum(bool(run.get("sut_outcome_reasons")) for run in admitted),
                "median_peak_occupancy": _median([run["peak_occupancy"] for run in runs]),
                "median_offered_msg_s": _median([run["rates_msg_s"]["offered"] for run in runs]),
                "median_accepted_msg_s": _median(
                    [run["rates_msg_s"]["accepted"] for run in runs]
                ),
                "median_processed_msg_s": _median(
                    [run["rates_msg_s"]["processed"] for run in runs]
                ),
                "median_drained_msg_s": _median([run["rates_msg_s"]["drained"] for run in runs]),
                "total_attempted": sum(value["attempted"] for value in counts),
                "total_delivered": sum(value["delivered"] for value in counts),
                "total_dropped": sum(value["dropped"] for value in counts),
                "total_dead_lettered": sum(value["dead_lettered"] for value in counts),
                "total_dlq_full": sum(value["dlq_full"] for value in counts),
                "total_dlq_closed": sum(value["dlq_closed"] for value in counts),
                "accounting_equation": runs[0]["accounting"]["equation"] if runs else None,
                "producer_progress": runs[0]["producer_progress"] if runs else None,
                "total_duplicates": sum(run["sequence"]["duplicates"] for run in runs),
                "rss_within_limit": len(runs) == len(admitted)
                and all(run["memory"]["within_limit"] for run in runs),
                "units": "occupancy ratio, messages/second, messages, boolean",
                "estimator": "run-level medians with exact policy-specific counter totals",
                "claim_boundary": "bounded internal queue under deterministic slow-consumer pressure",
                "thesis_evidence": canonical,
            }
        )
    return pd.DataFrame(rows)


def swap3_table(runs: list[dict]) -> pd.DataFrame:
    strategies = (
        "wafer-hotswap",
        "wafer-restart",
        "ekuiper-rule-update",
        "ekuiper-make-before-break",
    )
    rules = declared_thresholds()
    dip_rule = rules["e-swap-3-dip"]
    lossless_rule = rules["e-swap-3-lossless"]
    grouped = {strategy: [] for strategy in strategies}
    for run in runs:
        strategy = run.get("strategy", run.get("condition"))
        if strategy not in grouped:
            raise ValueError(f"unexpected E-Swap-3 strategy: {strategy}")
        grouped[str(strategy)].append(run)
    rows = []
    for strategy, admitted in grouped.items():
        indices = {int(run.get("run_index", 0)) for run in admitted}
        if len(admitted) != 30 or indices != set(range(1, 31)):
            raise ValueError(f"{strategy} requires 30 independent runs")
        values = [run for run in admitted if not _stopped_early(run)]
        dips = [float(run["dip_percent"]) for run in values]
        placebo_dips = [float(run["placebo_dip_percent"]) for run in values]
        interruptions = [float(run["interruption_ns"]) for run in values]
        actions = [float(run["action_duration_ns"]) for run in values]
        recoveries = [float(run["recovery_ns"]) for run in values]
        dip_low, dip_high = _ci(dips) if values else (None, None)
        placebo_low, placebo_high = _ci(placebo_dips) if values else (None, None)
        interruption_low, interruption_high = _ci(interruptions) if values else (None, None)
        action_low, action_high = _ci(actions) if values else (None, None)
        recovery_low, recovery_high = _ci(recoveries) if values else (None, None)
        lossless_runs = sum(
            int(run["loss"]) == 0 and int(run["duplicates"]) == 0 for run in values
        )
        if strategy == "wafer-hotswap":
            dip = bound_verdict(
                "dip",
                dip_rule,
                bootstrap_ci(np.asarray(dips), ci=dip_rule.interval) if dips else None,
                estimate=_median(dips),
            )
            verdict = combined_verdict(
                dip["dip_verdict"], count_verdict(lossless_rule, len(admitted) - lossless_runs)
            )
        else:
            dip, verdict = no_verdict("dip"), None
        rows.append(
            {
                "strategy": strategy,
                "N_runs": len(admitted),
                "median_dip_percent": _median(dips),
                "dip_ci95_low_percent": dip_low,
                "dip_ci95_high_percent": dip_high,
                "median_placebo_dip_percent": _median(placebo_dips),
                "placebo_dip_ci95_low_percent": placebo_low,
                "placebo_dip_ci95_high_percent": placebo_high,
                "median_interruption_ns": _median(interruptions),
                "interruption_ci95_low_ns": interruption_low,
                "interruption_ci95_high_ns": interruption_high,
                "median_action_duration_ns": _median(actions),
                "action_duration_ci95_low_ns": action_low,
                "action_duration_ci95_high_ns": action_high,
                "median_recovery_ns": _median(recoveries),
                "recovery_ci95_low_ns": recovery_low,
                "recovery_ci95_high_ns": recovery_high,
                "right_censored_runs": sum(
                    bool(run["recovery_right_censored"]) for run in values
                ),
                "total_loss": sum(int(run["loss"]) for run in values),
                "total_duplicates": sum(int(run["duplicates"]) for run in values),
                "lossless_runs": lossless_runs,
                "runs_stopped_early": len(admitted) - len(values),
                "zero_loss_and_duplication": lossless_runs == len(admitted),
                **dip,
                "verdict": verdict,
                "units": "percent, nanoseconds, messages",
                "estimator": "run-level median with bootstrap 95% CI over runs that kept running; lossless runs out of all admitted runs; dip verdict from the one-sided 95% upper bound over runs; placebo dip is the same estimator 6 s before the action, a descriptive noise floor with no verdict",
                "threshold": (
                    f"one-sided 95% upper bound of the median dip < {dip_rule.value:g} percent; zero loss; zero duplication"
                    if strategy == "wafer-hotswap"
                    else "measured comparator; no predeclared pass threshold"
                ),
                "claim_boundary": "event-aligned output disruption; stateless action",
                "thesis_evidence": True,
            }
        )
    return pd.DataFrame(rows)


SWAP_PHASES = (
    "compile_ns",
    "instantiate_ns",
    "signal_ns",
    "replacement_adopted_ns",
    "first_post_replacement_local_outcome_ns",
)
SWAP_EVENT_CLASSES = {"compiled": "first-use", "memory_hit": "cached", "disk_hit": "cached"}
SWAP_METRICS = (*SWAP_PHASES, "phase_total_ns", "http_total_ns", "sink_observed_output_gap_ns")
SWAP_TAIL_METRICS = ("phase_total_ns", "sink_observed_output_gap_ns")
SWAP_SESSION_RUNS = 10
SWAP_SESSION_EVENTS = 50


def _swap_session_runs(records: list[dict], experiment: str, *, canonical: bool) -> list[dict]:
    runs = sorted(records, key=lambda record: int(record.get("run_index", 0)))
    indices = [int(run.get("run_index", 0)) for run in runs]
    if len(set(indices)) != len(indices):
        raise ValueError(f"{experiment} contains duplicate run identity")
    if canonical and indices != list(range(1, SWAP_SESSION_RUNS + 1)):
        raise ValueError(f"{experiment} requires {SWAP_SESSION_RUNS} independent runs")
    return runs


def _swap_event_class(compile_cache: object, event_index: object) -> str:
    event_class = SWAP_EVENT_CLASSES.get(compile_cache)
    if event_class is None:
        raise ValueError(
            f"swap event {event_index} has unknown compile cache outcome {compile_cache!r}"
        )
    return event_class


def _median_over_runs(metric: str, run_values: list[float]) -> dict:
    name = metric.removesuffix("_ns")
    low, high = _ci(run_values)
    return {
        f"median_{metric}": float(np.median(run_values)),
        f"{name}_ci95_low_ns": low,
        f"{name}_ci95_high_ns": high,
    }


def swap_phase_table(runs: list[dict], *, canonical: bool = True) -> pd.DataFrame:
    """E-Swap-1/6: internal phases, HTTP duration and sink gap per compile-cache class.

    ``runs`` are ``hotswap-analysis.json`` documents with ``run_index`` added.
    Each run is first reduced to the median of its events in a class, and its
    cached swaps also to their p95 phase total and sink gap; the table gives the
    median of those run values with a bootstrap 95% CI over runs. A run the system
    under test stopped early (a failed swap, a runtime exit) has no phases and is
    counted in its own row, so the failure is reported rather than dropped.
    """
    session = _swap_session_runs(runs, "E-Swap-1", canonical=canonical)
    reasons = ", ".join(
        sorted({reason for run in session for reason in run.get("sut_outcome_reasons", ())})
    )
    by_class: dict[str, list[list[dict]]] = {"first-use": [], "cached": []}
    for run in session:
        if _stopped_early(run):
            continue
        if canonical and len(run["events"]) != SWAP_SESSION_EVENTS:
            raise ValueError(
                f"E-Swap-1 run {run['run_index']} requires {SWAP_SESSION_EVENTS} nested swap events"
            )
        classes: dict[str, list[dict]] = {"first-use": [], "cached": []}
        first_use = []
        for event in run["events"]:
            values = {phase: event[phase] for phase in SWAP_PHASES}
            values |= {
                "phase_total_ns": sum(values.values()),
                "http_total_ns": event["http_total_ns"],
                "sink_observed_output_gap_ns": event["sink_observed_output_gap_ns"],
            }
            event_class = _swap_event_class(event.get("compile_cache"), event.get("event_index"))
            classes[event_class].append(values)
            if event_class == "first-use":
                first_use.append(event.get("event_index"))
        if canonical and first_use != [0]:
            raise ValueError(
                f"E-Swap-1 run {run['run_index']} requires exactly one first-use swap event, its first"
            )
        for event_class, events in classes.items():
            if events:
                by_class[event_class].append(events)
    rows = []
    for event_class, runs_events in by_class.items():
        if not runs_events:
            continue
        row = {
            "experiment": "e-swap-1",
            "event_class": event_class,
            "N_runs": len(runs_events),
            "N_nested_events": sum(len(events) for events in runs_events),
        }
        for metric in SWAP_METRICS:
            row |= _median_over_runs(
                metric,
                [float(np.median([event[metric] for event in events])) for events in runs_events],
            )
        if event_class == "cached":
            for metric in SWAP_TAIL_METRICS:
                row |= _median_over_runs(
                    f"run_p95_{metric}",
                    [float(np.percentile([event[metric] for event in events], 95)) for events in runs_events],
                )
        rows.append(
            row
            | {
                "sut_outcome_reasons": reasons,
                "units": "nanoseconds, runs, swap events",
                "estimator": "median over runs of each run's median per compile-cache class and of each run's p95 cached phase total and sink gap, each with a bootstrap 95% CI over runs; the first-use class holds the one compiling swap of each run",
                "claim_boundary": "internal phases, HTTP duration and sink-observed gap are separate measurements; queued output can hide internal disruption from the sink",
                "thesis_evidence": canonical,
            }
        )
    stopped = [run for run in session if _stopped_early(run)]
    if stopped:
        rows.append(
            {
                "experiment": "e-swap-1",
                "event_class": "stopped early",
                "N_runs": len(stopped),
                "N_nested_events": 0,
                "sut_outcome_reasons": reasons,
                "units": "runs",
                "estimator": "runs the system under test stopped before their swap evidence was complete",
                "claim_boundary": "a failed swap or runtime exit fails the swap criteria; no phase durations exist",
                "thesis_evidence": canonical,
            }
        )
    return pd.DataFrame(rows)


def swap_sequence_table(records: list[dict], *, canonical: bool = True) -> pd.DataFrame:
    """E-Swap-2: loss and duplication in each E-Swap-1 run, then summed over runs.

    Each record is one run's ``sequence.csv`` counts under ``sequence`` with its
    ``run_index`` and ``sut_outcome_reasons``. A run the system under test stopped
    early has no counts and is not lossless. The last row, ``run == "all"``, holds
    the totals.
    """
    counts = ("expected", "received", "loss", "duplicates")
    rows = []
    for record in _swap_session_runs(records, "E-Swap-2", canonical=canonical):
        if _stopped_early(record):
            rows.append({"run": str(record["run_index"]), "runs_stopped_early": 1})
            continue
        sequence = record["sequence"]
        rows.append(
            {
                "run": str(record["run_index"]),
                "runs_stopped_early": 0,
                "expected": int(sequence["expected"]),
                "received": int(sequence["received"]),
                "loss": int(sequence["gaps"]),
                "duplicates": int(sequence["duplicates"]),
            }
        )
    if not rows:
        return pd.DataFrame()
    complete = [row for row in rows if not row["runs_stopped_early"]]
    rows.append(
        {"run": "all", "runs_stopped_early": len(rows) - len(complete)}
        | {field: sum(row[field] for row in complete) for field in counts}
    )
    table = pd.DataFrame(rows)
    return table.assign(
        lossless=(table.runs_stopped_early == 0)
        & (table.loss == 0)
        & (table.duplicates == 0)
        & (table.expected == table.received),
        units="messages",
        estimator="exact full-run sequence counts of each run and their sum over the runs that were not stopped early",
        threshold="zero loss and zero duplication in every run",
        claim_boundary="stateless swaps; E-Swap-2 reads the E-Swap-1 runs and adds no independent N",
        thesis_evidence=canonical,
    )


def bucket_band(offsets_ns: list[int], runs: list[list[float]]) -> pd.DataFrame:
    """Per-bucket median and IQR across runs of an event-aligned rate series."""
    if not runs or any(len(run) != len(offsets_ns) for run in runs):
        raise ValueError("every run needs one value per bucket")
    values = np.asarray(runs, dtype=float)
    p25, median, p75 = np.percentile(values, [25, 50, 75], axis=0)
    return pd.DataFrame(
        {
            "offset_s": np.asarray(offsets_ns, dtype=float) / 1e9,
            "N_runs": len(runs),
            "median": median,
            "p25": p25,
            "p75": p75,
        }
    )


def failed_replacement_table(records: list[dict], *, canonical: bool = True) -> pd.DataFrame:
    """E-Swap-5: one row per run, with its first-use rollback apart from its cached ones.

    A run the system under test stopped early has no rollback durations; its row
    counts the requests that still rolled back.
    """
    rows = []
    for record in _swap_session_runs(records, "E-Swap-5", canonical=canonical):
        if record.get("condition") != "process-trap-rollback":
            raise ValueError("E-Swap-5 run identity is invalid")
        reasons = ", ".join(record.get("sut_outcome_reasons", ()))
        if _stopped_early(record):
            requests = record.get("requests") or []
            rows.append(
                {
                    "experiment": "e-swap-5",
                    "condition": "process-trap-rollback",
                    "run_index": record["run_index"],
                    "N_runs": 1,
                    "N_nested_events": len(requests),
                    "rolled_back_events": sum(
                        request.get("http_status") == 200
                        and isinstance(request.get("body"), dict)
                        and request["body"].get("status") == "rolled_back"
                        for request in requests
                    ),
                    "all_rolled_back": False,
                    "stopped_early": True,
                    "sut_outcome_reasons": reasons,
                    "post_rollback_continuity": False,
                    "units": "rollback events, runs",
                    "estimator": "a run the system under test stopped early; no rollback durations",
                    "threshold": "50 successful rollbacks; post-rollback output; zero loss and duplication",
                    "claim_boundary": "failed replacement rollback and observed continuity without a successful v2 transition",
                    "thesis_evidence": canonical,
                }
            )
            continue
        requests = record.get("requests")
        rollback = record.get("rollback")
        continuity = record.get("continuity")
        sequence = record.get("sequence")
        if (
            not isinstance(requests, list)
            or not isinstance(rollback, dict)
            or not isinstance(continuity, dict)
            or not isinstance(sequence, dict)
        ):
            raise ValueError("E-Swap-5 analysis input is malformed")
        validate_swap5_artifacts(requests, rollback, continuity, sequence)
        durations = {"first-use": [], "cached": []}
        first_use = []
        for request, event in zip(requests, rollback["events"], strict=True):
            event_class = _swap_event_class(request["body"].get("compile_cache"), event["event_index"])
            durations[event_class].append(event["rollback_ns"])
            if event_class == "first-use":
                first_use.append(event["event_index"])
        if canonical and first_use != [0]:
            raise ValueError(
                f"E-Swap-5 run {record['run_index']} requires exactly one first-use rollback event, its first"
            )
        rows.append(
            {
                "experiment": "e-swap-5",
                "condition": "process-trap-rollback",
                "run_index": record["run_index"],
                "N_runs": 1,
                "N_nested_events": len(rollback["events"]),
                "rolled_back_events": rollback["rolled_back"],
                "all_rolled_back": True,
                "stopped_early": False,
                "sut_outcome_reasons": reasons,
                "median_first_use_rollback_ns": (
                    float(np.median(durations["first-use"])) if durations["first-use"] else None
                ),
                "median_cached_rollback_ns": (
                    float(np.median(durations["cached"])) if durations["cached"] else None
                ),
                "max_rollback_ns": float(max(durations["first-use"] + durations["cached"])),
                "post_rollback_messages": continuity["messages_after_final_rollback"],
                "post_rollback_continuity": continuity["output_observed_after_final_rollback"],
                "total_loss": sequence["gaps"],
                "total_duplicates": sequence["duplicates"],
                "units": "nanoseconds, messages, runs, rollback events",
                "estimator": "one complete run; the first-use (compiling) rollback apart from the median of the cached rollbacks",
                "threshold": "50 successful rollbacks; post-rollback output; zero loss and duplication",
                "claim_boundary": "failed replacement rollback and observed continuity without a successful v2 transition",
                "thesis_evidence": canonical,
            }
        )
    return pd.DataFrame(rows)


def failed_replacement_summary(runs: pd.DataFrame) -> pd.DataFrame:
    """E-Swap-5 over runs: medians of the per-run rollback durations and exact totals.

    ``runs`` is the per-run table from :func:`failed_replacement_table`.
    """
    if runs.empty:
        return pd.DataFrame()
    complete = runs[~runs.stopped_early.astype(bool)]
    row = {
        "experiment": "e-swap-5",
        "condition": "process-trap-rollback",
        "N_runs": len(runs),
        "N_nested_events": int(runs.N_nested_events.sum()),
        "rolled_back_events": int(runs.rolled_back_events.sum()),
        "all_rolled_back": bool(runs.all_rolled_back.all()),
        "runs_stopped_early": int(runs.stopped_early.sum()),
        "sut_outcome_reasons": ", ".join(
            sorted({reason for value in runs.sut_outcome_reasons for reason in value.split(", ") if reason})
        ),
    }
    for metric in ("first_use_rollback_ns", "cached_rollback_ns"):
        values = complete[f"median_{metric}"].dropna().astype(float).tolist()
        row |= _median_over_runs(metric, values) if values else {f"median_{metric}": None}
    return pd.DataFrame(
        [
            row
            | {
                "max_rollback_ns": None if complete.empty else float(complete.max_rollback_ns.max()),
                "runs_with_post_rollback_output": int(runs.post_rollback_continuity.astype(bool).sum()),
                "total_loss": int(complete.total_loss.sum()),
                "total_duplicates": int(complete.total_duplicates.sum()),
                "units": "nanoseconds, messages, runs, rollback events",
                "estimator": "median over runs of each run's first-use rollback and of its cached-rollback median, with bootstrap 95% CIs over runs; counts are exact sums over the runs that were not stopped early",
                "threshold": "50 successful rollbacks, post-rollback output, zero loss and zero duplication in every run",
                "claim_boundary": "failed replacement rollback and observed continuity without a successful v2 transition",
                "thesis_evidence": bool(runs.thesis_evidence.all()),
            }
        ]
    )


def swap4_table(runs: list[dict]) -> pd.DataFrame:
    """E-Swap-4 over every admitted run; a run that lost, duplicated or stopped early fails the zero-loss criterion."""
    indices = {int(run.get("run_index", 0)) for run in runs}
    if len(runs) != 30 or indices != set(range(1, 31)):
        raise ValueError("E-Swap-4 requires 30 independent runs")
    complete = [run for run in runs if not _stopped_early(run)]
    if any(run.get("successful_swaps") != 1 for run in complete):
        raise ValueError("E-Swap-4 requires one successful swap per run")
    if any(run.get("drain_right_censored") is not False for run in complete):
        raise ValueError("E-Swap-4 requires complete drain evidence")
    required_drain = {
        "primary_received_events",
        "drain_received_events",
        "drain_first_offset_ns",
        "drain_last_offset_ns",
        "drain_duration_after_window_ns",
        "max_arrival_offset_ns",
    }
    if any(not required_drain <= run.keys() for run in complete):
        raise ValueError("E-Swap-4 lacks run-level drain evidence")
    rules = declared_thresholds()
    gap_rule = rules["e-swap-4-p95-gap"]
    lossless_rule = rules["e-swap-4-lossless"]
    gaps = sorted(float(run["sink_observed_output_gap_ns"]) for run in complete)
    low, high = _ci(gaps) if gaps else (None, None)
    phase_medians = {
        f"median_{phase}": _median(
            [run["internal_swap_phases_ns"][phase] for run in complete]
        )
        for phase in (
            "compile_ns",
            "instantiate_ns",
            "signal_ns",
            "replacement_adopted_ns",
            "first_post_replacement_local_outcome_ns",
        )
    }
    lossless_runs = sum(
        int(run["loss"]) == 0 and int(run["sequence"]["duplicates"]) == 0 for run in complete
    )
    gap = bound_verdict(
        "p95_gap",
        gap_rule,
        bootstrap_ci(np.asarray(gaps), ci=gap_rule.interval, statistic=_nearest_rank_p95)
        if gaps
        else None,
        estimate=_nearest_rank_p95(gaps) if gaps else None,
    )
    return pd.DataFrame(
        [
            {
                "experiment": "e-swap-4",
                "condition": "burst-2x",
                "N_runs": len(runs),
                "N_events": len(gaps),
                "median_sink_gap_ns": _median(gaps),
                "iqr_sink_gap_ns": float(
                    np.percentile(gaps, 75) - np.percentile(gaps, 25)
                )
                if gaps
                else None,
                "p95_sink_gap_ns": _nearest_rank_p95(gaps) if gaps else None,
                "bootstrap_median_ci95_low_ns": low,
                "bootstrap_median_ci95_high_ns": high,
                **phase_medians,
                "total_loss": sum(int(run["loss"]) for run in complete),
                "total_duplicates": sum(
                    int(run["sequence"]["duplicates"]) for run in complete
                ),
                "lossless_runs": lossless_runs,
                "runs_stopped_early": len(runs) - len(complete),
                "zero_loss_and_duplication": lossless_runs == len(runs),
                "runs_with_drain_arrivals": sum(
                    int(run["drain_received_events"]) > 0 for run in complete
                ),
                "median_primary_received_events": _median(
                    [run["primary_received_events"] for run in complete]
                ),
                "median_drain_received_events": _median(
                    [run["drain_received_events"] for run in complete]
                ),
                "max_drain_arrival_offset_ns": max(
                    (
                        int(run["drain_last_offset_ns"])
                        for run in complete
                        if run["drain_last_offset_ns"] is not None
                    ),
                    default=None,
                ),
                "max_drain_duration_after_window_ns": max(
                    (int(run["drain_duration_after_window_ns"]) for run in complete),
                    default=None,
                ),
                "max_arrival_offset_ns": max(
                    (int(run["max_arrival_offset_ns"]) for run in complete), default=None
                ),
                "drain_right_censored_runs": 0,
                **gap,
                "verdict": combined_verdict(
                    gap["p95_gap_verdict"], count_verdict(lossless_rule, len(runs) - lossless_runs)
                ),
                "units": "nanoseconds, messages, runs",
                "estimator": "one sink gap and one internal-phase vector per run that kept running; source-origin primary/drain completion counts; lossless runs out of all admitted runs; gap verdict from the one-sided 95% upper bound of the nearest-rank p95 over runs",
                "threshold": (
                    f"one-sided 95% upper bound of the across-run p95 sink gap < {gap_rule.value / 1_000_000:g} ms; "
                    "zero full-run loss; zero duplication; no receive at or after 130 s"
                ),
                "claim_boundary": "one stateless swap centered in one source-driven burst per run; drain excluded from t=60 disruption estimator",
                "thesis_evidence": True,
            }
        ]
    )
