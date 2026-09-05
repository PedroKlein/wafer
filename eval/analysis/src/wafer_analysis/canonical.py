"""Run-level summaries for final canonical evaluation artifacts."""

from __future__ import annotations

import math
from collections.abc import Iterable

import numpy as np
import pandas as pd

from .stats import bootstrap_ci, cliffs_delta

FINAL_VISUAL_MANIFEST = (
    {
        "experiment": "e-perf-1/2",
        "name": "target-load-latency",
        "metric_group": "run-level-latency",
    },
    {
        "experiment": "e-perf-7",
        "name": "metering-ablation",
        "metric_group": "metering-effect",
    },
    {
        "experiment": "e-perf-10",
        "name": "offered-achieved",
        "metric_group": "offered-versus-achieved-rate",
    },
    {"experiment": "e-perf-10", "name": "loss", "metric_group": "loss"},
    {"experiment": "e-perf-10", "name": "p99", "metric_group": "p99-latency"},
    {
        "experiment": "e-perf-10",
        "name": "delivery-ceiling",
        "metric_group": "delivery-ceiling",
    },
    {
        "experiment": "e-perf-10",
        "name": "normalized-knee",
        "metric_group": "normalized-p99-knee",
    },
    {
        "experiment": "e-perf-10",
        "name": "support-limitation",
        "metric_group": "mqtt-support-path-limitation",
    },
    {
        "experiment": "e-swap-1/4",
        "name": "internal-phases",
        "metric_group": "internal-swap-phases",
    },
    {
        "experiment": "e-swap-1/4",
        "name": "sink-gap",
        "metric_group": "sink-observed-gap",
    },
    {
        "experiment": "e-swap-3",
        "name": "event-dip",
        "metric_group": "event-aligned-dip",
    },
    {
        "experiment": "e-swap-3",
        "name": "action-duration",
        "metric_group": "action-duration",
    },
    {"experiment": "e-swap-3", "name": "recovery", "metric_group": "recovery"},
    {
        "experiment": "e-swap-3/4",
        "name": "sequence-integrity",
        "metric_group": "sequence-integrity",
    },
    {
        "experiment": "e-swap-4",
        "name": "burst-pause",
        "metric_group": "burst-one-event-per-run",
    },
)

_ALLOWED_METRIC_GROUPS = {row["metric_group"] for row in FINAL_VISUAL_MANIFEST}


def validate_visual_manifest(manifest: Iterable[dict]) -> None:
    seen: set[tuple[str, str]] = set()
    for row in manifest:
        key = (str(row.get("experiment", "")), str(row.get("name", "")))
        metric = row.get("metric_group")
        if not all(key) or key in seen:
            raise ValueError("visual manifest names must be unique and non-empty")
        if metric not in _ALLOWED_METRIC_GROUPS:
            raise ValueError(f"combined or unknown metric group: {metric}")
        seen.add(key)


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


def target_latency_table(records: list[dict]) -> pd.DataFrame:
    conditions = ("wafer", "native", "ekuiper")
    grouped = _require_runs(records, conditions)
    reference = np.asarray([run["p95_ns"] for run in grouped["ekuiper"]], dtype=float)
    reference_median = float(np.median(reference))
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
        delta, magnitude = cliffs_delta(values, reference)
        intended = sum(int(run["intended_messages"]) for run in condition_runs)
        received = sum(int(run["received_unique"]) for run in condition_runs)
        pooled_loss = (intended - received) / intended
        mean_achieved_ratio = float(
            np.mean([run["achieved_ratio"] for run in condition_runs])
        )
        total_duplicates = sum(int(run["duplicates"]) for run in condition_runs)
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
                "mean_achieved_ratio": mean_achieved_ratio,
                "total_duplicates": total_duplicates,
                "delivery_good": pooled_loss <= 0.01
                and mean_achieved_ratio >= 0.99
                and total_duplicates == 0,
                "reference_condition": "ekuiper",
                "median_ratio_vs_reference": float(np.median(values))
                / reference_median,
                "cliffs_delta_vs_reference": delta,
                "effect_magnitude": magnitude,
                "units": "nanoseconds, messages/second, fraction, messages",
                "estimator": "median run p95 and achieved rate with bootstrap 95% CI; pooled loss; mean achieved ratio",
                "threshold": "median(WAFER p95) / median(eKuiper p95) <= 2.0; pooled loss <= 0.01; mean achieved/offered >= 0.99; zero duplicates",
                "claim_boundary": "matched 1,000 msg/s target load; not capacity",
                "thesis_evidence": True,
            }
        )
    return pd.DataFrame(rows)


def metering_table(records: list[dict]) -> pd.DataFrame:
    conditions = ("neither", "fuel-only", "epoch-only", "both")
    grouped = _require_runs(records, conditions)
    reference = np.asarray([run["p95_ns"] for run in grouped["neither"]], dtype=float)
    rows = []
    for condition in conditions:
        values = np.asarray([run["p95_ns"] for run in grouped[condition]], dtype=float)
        low, high = bootstrap_ci(values)
        delta, magnitude = cliffs_delta(values, reference)
        paired_difference = values - reference
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
                "cliffs_delta_vs_neither": delta,
                "effect_magnitude": magnitude,
                "units": "nanoseconds",
                "estimator": "median run p95 difference and ratio with bootstrap 95% CI",
                "claim_boundary": "run-level metering ablation; no per-message inference",
                "thesis_evidence": True,
            }
        )
    return pd.DataFrame(rows)


def capacity_tables(summary: dict) -> tuple[pd.DataFrame, pd.DataFrame]:
    if (
        summary.get("schema_version") != 1
        or summary.get("experiment") != "e-perf-10"
        or summary.get("thesis_evidence") is not True
        or summary.get("sample_unit") != "run"
        or summary.get("required_runs_per_rate") != 30
    ):
        raise ValueError("malformed final capacity summary")
    if summary.get("rate_points_msg_s") != [1_000, 4_000, 8_000, 15_000, 16_000]:
        raise ValueError("final capacity summary uses the wrong common rate grid")
    systems = summary.get("systems")
    if not isinstance(systems, dict) or set(systems) != {
        "mqtt-loopback",
        "native",
        "wafer",
        "ekuiper",
    }:
        raise ValueError("final capacity summary requires all four systems")
    rate_rows = []
    boundary_rows = []
    for system, result in systems.items():
        rates = result.get("rates")
        if not result.get("complete") or not isinstance(rates, list) or len(rates) != 5:
            raise ValueError(f"{system} capacity summary is incomplete")
        if [rate.get("rate_msg_s") for rate in rates] != summary["rate_points_msg_s"]:
            raise ValueError(f"{system} capacity rates differ from the common grid")
        for rate in rates:
            if rate.get("run_count") != 30:
                raise ValueError(f"{system} rate requires 30 independent runs")
            run_summary = rate.get("run_summary")
            normalized = rate.get("normalized_p99")
            if not isinstance(run_summary, dict) or not isinstance(normalized, dict):
                raise TypeError(f"{system} rate summary is malformed")
            try:
                achieved = run_summary["achieved_rate_msg_s"]
                p99 = run_summary["p99_ns"]
                achieved_ci = achieved["bootstrap_median_ci95"]
                p99_ci = p99["bootstrap_median_ci95"]
                normalized_ci = normalized["bootstrap_median_ci95"]
            except (KeyError, TypeError) as error:
                raise ValueError(f"{system} rate summary is malformed") from error
            classification = rate.get("classification")
            if classification not in {
                "good",
                "bad",
                "support-confounded",
                "incomplete",
            }:
                raise ValueError(f"{system} rate classification is invalid")
            rate_rows.append(
                {
                    "system": system,
                    "offered_rate_msg_s": int(rate["rate_msg_s"]),
                    "N_runs": 30,
                    "median_achieved_rate_msg_s": float(achieved["median"]),
                    "achieved_ci95_low_msg_s": float(achieved_ci[0]),
                    "achieved_ci95_high_msg_s": float(achieved_ci[1]),
                    "pooled_loss": float(rate["pooled_loss"]),
                    "mean_achieved_ratio": float(rate["mean_achieved_ratio"]),
                    "median_p99_ns": float(p99["median"]),
                    "p99_ci95_low_ns": float(p99_ci[0]),
                    "p99_ci95_high_ns": float(p99_ci[1]),
                    "median_normalized_p99": float(normalized["median"]),
                    "normalized_p99_ci95_low": float(normalized_ci[0]),
                    "normalized_p99_ci95_high": float(normalized_ci[1]),
                    "classification": classification,
                    "delivery_good": classification == "good",
                    "support_confounded": classification == "support-confounded",
                    "units": "messages/second, fraction, nanoseconds",
                    "estimator": "pooled loss; mean achieved ratio; median run p99",
                    "thesis_evidence": True,
                }
            )
        support = result["support_censoring"]
        boundary_rows.append(
            {
                "system": system,
                "delivery_ceiling_msg_s": result["delivery_ceiling"]["rate_msg_s"],
                "delivery_ceiling_censoring": result["delivery_ceiling"]["censoring"],
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
                "units": "messages/second",
                "claim_boundary": "co-located gateway envelope; SUT ceilings stop at the MQTT support path",
                "thesis_evidence": True,
            }
        )
    return pd.DataFrame(rate_rows), pd.DataFrame(boundary_rows)


def swap3_table(runs: list[dict]) -> pd.DataFrame:
    strategies = ("wafer-hotswap", "wafer-restart", "ekuiper-restart")
    grouped = {strategy: [] for strategy in strategies}
    for run in runs:
        strategy = run.get("strategy")
        if strategy not in grouped:
            raise ValueError(f"unexpected E-Swap-3 strategy: {strategy}")
        grouped[str(strategy)].append(run)
    rows = []
    for strategy, values in grouped.items():
        indices = {int(run.get("run_index", 0)) for run in values}
        if len(values) != 30 or indices != set(range(1, 31)):
            raise ValueError(f"{strategy} requires 30 independent runs")
        dips = [float(run["dip_percent"]) for run in values]
        interruptions = [float(run["interruption_ns"]) for run in values]
        actions = [float(run["action_duration_ns"]) for run in values]
        recoveries = [float(run["recovery_ns"]) for run in values]
        dip_low, dip_high = _ci(dips)
        interruption_low, interruption_high = _ci(interruptions)
        action_low, action_high = _ci(actions)
        recovery_low, recovery_high = _ci(recoveries)
        rows.append(
            {
                "strategy": strategy,
                "N_runs": len(values),
                "median_dip_percent": float(np.median(dips)),
                "dip_ci95_low_percent": dip_low,
                "dip_ci95_high_percent": dip_high,
                "median_interruption_ns": float(np.median(interruptions)),
                "interruption_ci95_low_ns": interruption_low,
                "interruption_ci95_high_ns": interruption_high,
                "median_action_duration_ns": float(np.median(actions)),
                "action_duration_ci95_low_ns": action_low,
                "action_duration_ci95_high_ns": action_high,
                "median_recovery_ns": float(np.median(recoveries)),
                "recovery_ci95_low_ns": recovery_low,
                "recovery_ci95_high_ns": recovery_high,
                "right_censored_runs": sum(
                    bool(run["recovery_right_censored"]) for run in values
                ),
                "total_loss": sum(int(run["loss"]) for run in values),
                "total_duplicates": sum(int(run["duplicates"]) for run in values),
                "units": "percent, nanoseconds, messages",
                "estimator": "run-level median with bootstrap 95% CI",
                "threshold": (
                    "upper bootstrap CI for median dip < 5%; zero loss; zero duplication"
                    if strategy == "wafer-hotswap"
                    else "measured comparator; no predeclared pass threshold"
                ),
                "claim_boundary": "event-aligned output disruption; stateless action",
                "thesis_evidence": True,
            }
        )
    return pd.DataFrame(rows)


def swap4_table(runs: list[dict]) -> pd.DataFrame:
    indices = {int(run.get("run_index", 0)) for run in runs}
    if len(runs) != 30 or indices != set(range(1, 31)):
        raise ValueError("E-Swap-4 requires 30 independent runs")
    if any(run.get("successful_swaps") != 1 for run in runs):
        raise ValueError("E-Swap-4 requires one successful swap per run")
    if any(run.get("drain_right_censored") is not False for run in runs):
        raise ValueError("E-Swap-4 requires complete drain evidence")
    required_drain = {
        "primary_received_events",
        "drain_received_events",
        "drain_first_offset_ns",
        "drain_last_offset_ns",
        "drain_duration_after_window_ns",
        "max_arrival_offset_ns",
    }
    if any(not required_drain <= run.keys() for run in runs):
        raise ValueError("E-Swap-4 lacks run-level drain evidence")
    gaps = sorted(float(run["sink_observed_output_gap_ns"]) for run in runs)
    low, high = _ci(gaps)
    phase_medians = {
        f"median_{phase}": float(
            np.median([run["internal_swap_phases_ns"][phase] for run in runs])
        )
        for phase in (
            "compile_ns",
            "instantiate_ns",
            "signal_ns",
            "ack_ns",
            "convergence_ns",
        )
    }
    return pd.DataFrame(
        [
            {
                "experiment": "e-swap-4",
                "condition": "burst-2x",
                "N_runs": len(runs),
                "N_events": len(gaps),
                "median_sink_gap_ns": float(np.median(gaps)),
                "iqr_sink_gap_ns": float(
                    np.percentile(gaps, 75) - np.percentile(gaps, 25)
                ),
                "p95_sink_gap_ns": gaps[math.ceil(len(gaps) * 0.95) - 1],
                "bootstrap_median_ci95_low_ns": low,
                "bootstrap_median_ci95_high_ns": high,
                **phase_medians,
                "total_loss": sum(int(run["loss"]) for run in runs),
                "total_duplicates": sum(
                    int(run["sequence"]["duplicates"]) for run in runs
                ),
                "runs_with_drain_arrivals": sum(
                    int(run["drain_received_events"]) > 0 for run in runs
                ),
                "median_primary_received_events": float(
                    np.median([run["primary_received_events"] for run in runs])
                ),
                "median_drain_received_events": float(
                    np.median([run["drain_received_events"] for run in runs])
                ),
                "max_drain_arrival_offset_ns": max(
                    (
                        int(run["drain_last_offset_ns"])
                        for run in runs
                        if run["drain_last_offset_ns"] is not None
                    ),
                    default=None,
                ),
                "max_drain_duration_after_window_ns": max(
                    int(run["drain_duration_after_window_ns"]) for run in runs
                ),
                "max_arrival_offset_ns": max(
                    int(run["max_arrival_offset_ns"]) for run in runs
                ),
                "drain_right_censored_runs": 0,
                "units": "nanoseconds, messages, runs",
                "estimator": "one sink gap and one internal-phase vector per run; source-origin primary/drain completion counts",
                "threshold": "across-run p95 sink gap < 100 ms; zero full-run loss; zero duplication; no receive at or after 130 s",
                "claim_boundary": "one stateless swap centered in one source-driven burst per run; drain excluded from t=60 disruption estimator",
                "thesis_evidence": True,
            }
        ]
    )
