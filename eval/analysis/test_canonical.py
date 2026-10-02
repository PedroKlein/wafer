from __future__ import annotations

import hashlib
import json
import random
from pathlib import Path
from statistics import NormalDist

import numpy as np
import pandas as pd
import pytest

from wafer_analysis.canonical import (
    containment_table,
    candidate_capacity_table,
    candidate_depth_table,
    candidate_payload_table,
    candidate_swap_tables,
    backpressure_table,
    bucket_band,
    branch_isolation_table,
    capacity_tables,
    ekuiper_profile_tables,
    failed_replacement_summary,
    failed_replacement_table,
    capacity_competitive_decision,
    density_table,
    depth_tables,
    metering_table,
    overhead_contrast_table,
    startup_table,
    payload_table,
    recovery_table,
    swap3_table,
    swap4_table,
    swap_phase_table,
    swap_sequence_table,
    TARGET_LOAD_CRITERIA,
    target_contrast_table,
    target_latency_table,
    validation_gate_table,
)
from wafer_analysis import paths
from wafer_analysis.focused import admitted_runs
from wafer_analysis.stats import bootstrap_ci, cliffs_delta, median_shift_ci
from wafer_analysis.verdicts import concordance_table


def percentile_runs(conditions: tuple[str, ...], n: int = 30) -> list[dict]:
    return [
        {
            "condition": condition,
            "run_index": run,
            "p50_ns": 100_000 + index * 1_000 + run,
            "p95_ns": 120_000 + index * 1_000 + run,
            "p99_ns": 140_000 + index * 1_000 + run,
            "intended_messages": 60_000,
            "received_unique": 60_000,
            "loss_fraction": 0.0,
            "achieved_rate_msg_s": 1_000.0,
            "achieved_ratio": 1.0,
            "duplicates": 0,
        }
        for index, condition in enumerate(conditions)
        for run in range(1, n + 1)
    ]


def test_target_latency_uses_runs_and_reports_ci_effect_threshold_and_boundary() -> (
    None
):
    table = target_latency_table(percentile_runs(("wafer", "native", "ekuiper")))
    assert table["N_runs"].tolist() == [30, 30, 30]
    assert table["units"].eq("nanoseconds, messages/second, fraction, messages").all()
    assert (
        table["estimator"]
        .str.contains("median run p95 and achieved rate with bootstrap 95% CI")
        .all()
    )
    assert table["ci95_low_ns"].notna().all() and table["ci95_high_ns"].notna().all()
    wafer = table.loc[table.condition == "wafer"].iloc[0]
    assert wafer["reference_condition"] == "ekuiper"
    assert wafer["cliffs_delta_vs_reference"] is not None
    assert wafer["cliffs_delta_ci95_low"] <= wafer["cliffs_delta_vs_reference"] <= wafer["cliffs_delta_ci95_high"]
    assert wafer["pooled_loss"] == 0
    assert wafer["pooled_loss_ci95_low"] == 0
    assert wafer["pooled_loss_ci95_high"] == 0
    assert wafer["mean_achieved_ratio"] == 1
    assert wafer["median_achieved_rate_msg_s"] == 1_000
    assert wafer["delivery_verdict"] == "PASS"
    assert wafer["p95_ratio_verdict"] == "PASS"
    assert wafer["verdict"] == "PASS"
    assert "pooled loss <= 0.01" in wafer["threshold"]
    assert wafer["claim_boundary"] == "matched 1,000 msg/s target load; not capacity"
    assert table["thesis_evidence"].eq(True).all()


def test_target_latency_ratio_and_effect_resample_run_pairs() -> None:
    block = {run: 100_000 * (1 + run % 5) for run in range(1, 31)}
    records = percentile_runs(("wafer", "native", "ekuiper"))
    for record in records:
        scale = 1.9 if record["condition"] == "wafer" else 1.0
        record["p95_ns"] = scale * block[record["run_index"]]

    table = target_latency_table(records).set_index("condition")

    wafer, native = table.loc["wafer"], table.loc["native"]
    assert (wafer["p95_ratio_verdict"], wafer["verdict"]) == ("PASS", "PASS")
    assert wafer["p95_ratio_ci_half_width"] == pytest.approx(0)
    assert (wafer["ratio_ci95_low"], wafer["ratio_ci95_high"]) == pytest.approx((1.9, 1.9))
    assert (native["ratio_ci95_low"], native["ratio_ci95_high"]) == pytest.approx((1, 1))
    assert (native["cliffs_delta_ci95_low"], native["cliffs_delta_ci95_high"]) == (0.0, 0.0)
    assert "over run pairs" in wafer["estimator"]


def test_target_latency_diagnostic_table_pairs_the_run_indices_both_systems_have() -> None:
    records = [
        record
        for record in percentile_runs(("wafer", "native", "ekuiper"), n=5)
        if (record["condition"], record["run_index"]) != ("ekuiper", 5)
    ]

    table = target_latency_table(records, canonical=False).set_index("condition")

    assert table["N_runs"].to_dict() == {"wafer": 5, "native": 5, "ekuiper": 4}
    assert table["thesis_evidence"].eq(False).all()
    assert table.loc["wafer", "p95_ratio_verdict"] == "PASS"
    assert table.loc["wafer", "median_ratio_vs_reference"] == pytest.approx(120_002.5 / 122_002.5)


BLOCKS = {run: 100_000.0 * (1 + run % 5) for run in range(1, 31)}


def contrast_runs(conditions: tuple[str, ...], wafer_p95_extra, n: int = 30) -> list[dict]:
    """Runs whose latency follows a block effect shared by every system of a run index."""
    return [
        {
            "condition": condition,
            "run_index": run,
            "p50_ns": (1.2 if condition == "wafer" else 1.0) * BLOCKS[run],
            "p95_ns": 2 * BLOCKS[run] + (wafer_p95_extra(run) if condition == "wafer" else 0),
        }
        for condition in conditions
        for run in range(1, n + 1)
    ]


def test_target_contrast_reports_the_paired_difference_and_its_minimum_detectable_size() -> None:
    def extra(run: int) -> int:
        return 10_000 if run % 2 else 30_000

    table = target_contrast_table(contrast_runs(("wafer", "native", "ekuiper"), extra))

    p95 = table.set_index("statistic").loc["p95"]
    differences = np.asarray([extra(run) for run in range(1, 31)], dtype=float)
    z = NormalDist().inv_cdf(0.975) + NormalDist().inv_cdf(0.8)
    assert table["statistic"].tolist() == ["p95", "p50"]
    assert (p95["condition"], p95["reference_condition"], p95["N_pairs"]) == ("wafer", "native", 30)
    assert p95["difference_ns"] == 20_000
    assert (p95["difference_ci95_low_ns"], p95["difference_ci95_high_ns"]) == bootstrap_ci(differences)
    assert p95["difference_ci_half_width_ns"] == pytest.approx(
        (p95["difference_ci95_high_ns"] - p95["difference_ci95_low_ns"]) / 2
    )
    assert p95["paired_sd_ns"] == pytest.approx(np.std(differences, ddof=1))
    assert p95["mdd_ns"] == pytest.approx(z * np.std(differences, ddof=1) / np.sqrt(30))
    assert "verdict" not in table and table["thesis_evidence"].eq(True).all()
    assert "power 0.80" in p95["estimator"]


def test_overhead_contrast_resamples_run_pairs_for_the_ratio() -> None:
    row = overhead_contrast_table(contrast_runs(("wafer", "native"), lambda run: 0)).iloc[0]

    assert (row["statistic"], row["N_pairs"]) == ("p50", 30)
    assert row["median_ratio"] == pytest.approx(1.2)
    assert (row["ratio_ci95_low"], row["ratio_ci95_high"]) == pytest.approx((1.2, 1.2))
    assert row["difference_ns"] == pytest.approx(0.2 * np.median(list(BLOCKS.values())))
    assert "no cross-architecture claim" in row["claim_boundary"]


def test_contrasts_need_complete_canonical_runs_but_pair_what_a_diagnostic_batch_has() -> None:
    with pytest.raises(ValueError, match="30 independent runs"):
        overhead_contrast_table(contrast_runs(("wafer", "native"), lambda run: 0, n=29))

    runs = [
        run
        for run in contrast_runs(("wafer", "native"), lambda run: 0, n=3)
        if (run["condition"], run["run_index"]) != ("native", 3)
    ]
    row = overhead_contrast_table(runs, canonical=False).iloc[0]
    assert (row["N_pairs"], row["thesis_evidence"]) == (2, False)
    single = overhead_contrast_table(runs[:1] + runs[3:4], canonical=False).iloc[0]
    assert single["N_pairs"] == 1
    assert pd.isna(single["paired_sd_ns"]) and pd.isna(single["mdd_ns"])
    assert overhead_contrast_table(runs[:2], canonical=False).empty


def test_contrasts_leave_out_a_run_the_system_stopped_early_and_its_partner() -> None:
    runs = contrast_runs(("wafer", "native"), lambda run: 0)
    runs[16] = {"condition": "wafer", "run_index": 17, "sut_outcome_reasons": ["runtime-exit"]}

    row = overhead_contrast_table(runs).iloc[0]

    assert (row["N_pairs"], row["runs_stopped_early"], row["thesis_evidence"]) == (29, 1, True)
    assert row["median_ratio"] == pytest.approx(1.2)
    assert "stopped early" in row["estimator"]
    assert target_contrast_table(contrast_runs(("wafer", "native", "ekuiper"), lambda run: 0))[
        "runs_stopped_early"
    ].eq(0).all()


def test_replication_concordance_reads_per_host_target_load_tables() -> None:
    pi = target_latency_table(percentile_runs(("wafer", "native", "ekuiper")))
    slower = percentile_runs(("wafer", "native", "ekuiper"))
    for record in slower:
        if record["condition"] == "wafer":
            record["p95_ns"] *= 3
    tables = {"rpi5": pi, "jetson": target_latency_table(slower), "x86": pi}

    table = concordance_table(tables, TARGET_LOAD_CRITERIA).set_index(["criterion", "condition", "host"])

    assert table.loc[("e-perf-1-p95-ratio", "wafer", "jetson"), "concordance"] == "opposite-direction"
    assert table.loc[("e-perf-1-p95-ratio", "wafer", "x86"), "concordance"] == "same-verdict"
    assert table.loc[("e-perf-1-p95-ratio", "wafer", "jetson"), "canonical_verdict"] == "PASS"
    assert table.loc[("e-perf-1-pooled-loss", "ekuiper", "jetson"), "concordance"] == "same-verdict"
    assert len(table) == (1 + 3 + 3 + 3) * 2
    assert pi.loc[pi.condition == "wafer", "verdict"].item() == "PASS"


def test_replication_concordance_shows_a_duplicate_that_flips_the_replication_verdict() -> None:
    pi = target_latency_table(percentile_runs(("wafer", "native", "ekuiper")))
    duplicated = percentile_runs(("wafer", "native", "ekuiper"))
    duplicated[4]["duplicates"] = 3
    jetson = target_latency_table(duplicated)

    table = concordance_table({"rpi5": pi, "jetson": jetson}, TARGET_LOAD_CRITERIA)

    wafer = table[(table.condition == "wafer") & (table.host == "jetson")].set_index("criterion")
    assert jetson.loc[jetson.condition == "wafer", "verdict"].item() == "FAIL"
    assert wafer.loc["e-perf-1-duplicates", "concordance"] == "opposite-direction"
    assert (wafer.loc["e-perf-1-duplicates", "canonical_estimate"], wafer.loc["e-perf-1-duplicates", "replication_estimate"]) == (0, 3)
    assert wafer.drop("e-perf-1-duplicates")["concordance"].eq("same-verdict").all()


def test_target_latency_rejects_missing_delivery_evidence() -> None:
    records = percentile_runs(("wafer", "native", "ekuiper"))
    del records[0]["achieved_ratio"]
    with pytest.raises(ValueError, match="lacks target-load delivery evidence"):
        target_latency_table(records)


def test_final_tables_reject_incomplete_independent_n() -> None:
    with pytest.raises(ValueError, match="30 independent runs"):
        target_latency_table(percentile_runs(("wafer", "native", "ekuiper"), n=29))
    with pytest.raises(ValueError, match="30 independent runs"):
        metering_table(
            percentile_runs(("neither", "fuel-only", "epoch-only", "both"), n=29)
        )


def test_metering_table_reports_difference_ratio_ci_and_nonparametric_effect() -> None:
    table = metering_table(
        percentile_runs(("neither", "fuel-only", "epoch-only", "both"))
    )
    assert set(table.condition) == {"neither", "fuel-only", "epoch-only", "both"}
    fuel = table.loc[table.condition == "fuel-only"].iloc[0]
    assert fuel["reference_condition"] == "neither"
    assert fuel["median_p95_difference_ns"] > 0
    assert fuel["median_p95_ratio"] > 1
    assert fuel["ratio_ci95_low"] > 1
    assert fuel["ratio_ci95_high"] > 1
    assert fuel["cliffs_delta_vs_neither"] is not None
    assert fuel["difference_ci95_low_ns"] is not None
    assert fuel["hodges_lehmann_shift_ns"] > 0
    assert fuel["shift_ci95_low_ns"] <= fuel["hodges_lehmann_shift_ns"] <= fuel["shift_ci95_high_ns"]
    assert fuel["cliffs_delta_ci95_low"] <= fuel["cliffs_delta_vs_neither"] <= fuel["cliffs_delta_ci95_high"]
    assert (
        fuel["claim_boundary"]
        == "run-level metering ablation; no per-message inference"
    )


def backpressure_records() -> list[dict]:
    records = []
    for policy in ("slow", "drop", "dead-letter"):
        for run_index in range(1, 31):
            counts = {
                "attempted": 1_000,
                "accepted": 1_000,
                "processed": 1_000,
                "delivered": 1_000,
                "dropped": 0,
                "dead_lettered": 0,
                "downstream_closed": 0,
                "dlq_full": 0,
                "dlq_closed": 0,
                "outstanding": 0,
            }
            if policy == "drop":
                counts.update(accepted=700, processed=700, delivered=700, dropped=300)
            elif policy == "dead-letter":
                counts.update(accepted=700, processed=700, delivered=700, dead_lettered=300)
            equations = {
                "slow": "attempted = delivered",
                "drop": "attempted = delivered + dropped",
                "dead-letter": "attempted = delivered + dead_lettered + dlq_full + dlq_closed",
            }
            records.append(
                {
                    "schema_version": 2,
                    "experiment": "e-backpressure",
                    "condition": policy,
                    "run_index": run_index,
                    "sample_unit": "run",
                    "policy": policy,
                    "queue": "slow",
                    "classification": "saturated-and-drained",
                    "threshold_crossed": True,
                    "recovered": True,
                    "peak_occupancy": 1.0,
                    "occupancy_threshold": 0.8,
                    "recovery_threshold": 0.1,
                    "counts": counts,
                    "rates_msg_s": {
                        "offered": 1_000.0,
                        "accepted": float(counts["accepted"]),
                        "processed": float(counts["processed"]),
                        "drained": 1_000.0,
                    },
                    "sequence": {
                        "offered": 1_000,
                        "received": counts["delivered"],
                        "gaps": 1_000 - counts["delivered"],
                        "duplicates": 0,
                    },
                    "accounting": {
                        "equation": equations[policy],
                        "reconciled": True,
                        "dlq_failures": {"full": 0, "closed": 0, "total": 0},
                    },
                    "producer_progress": "backpressured" if policy == "slow" else "nonblocking",
                    "memory": {"within_limit": True},
                }
            )
    return records


def test_backpressure_table_renders_policy_specific_accounting() -> None:
    table = backpressure_table(backpressure_records())
    assert table["policy"].tolist() == ["slow", "drop", "dead-letter"]
    assert table["N_runs"].eq(30).all()
    slow, drop, dead_letter = table.to_dict("records")
    assert slow["total_attempted"] == slow["total_delivered"] == 30_000
    assert drop["total_attempted"] == drop["total_delivered"] + drop["total_dropped"]
    assert dead_letter["total_attempted"] == (
        dead_letter["total_delivered"] + dead_letter["total_dead_lettered"]
    )
    assert set(table["producer_progress"]) == {"backpressured", "nonblocking"}
    assert table["rss_within_limit"].all()
    assert table["thesis_evidence"].all()
    assert not backpressure_table(backpressure_records(), canonical=False)["thesis_evidence"].any()


def test_backpressure_table_rejects_each_policy_malformed_accounting() -> None:
    for policy in ("slow", "drop", "dead-letter"):
        records = backpressure_records()
        record = next(item for item in records if item["policy"] == policy)
        record["counts"]["attempted"] += 1
        with pytest.raises(ValueError, match="counters do not reconcile|policy accounting|not lossless"):
            backpressure_table(records)


FROZEN_CAPACITY_GRID = (1_000, 4_000, 8_000, 15_000, 16_000)
DELIVERY_COUNTERS = {"good": (0.0, 1.0), "bad": (0.02, 0.97)}


def capacity_summary(grid: tuple[int, ...] = FROZEN_CAPACITY_GRID) -> dict:
    top = grid[-1]
    systems = {}
    for system in ("mqtt-loopback", "native", "wafer", "ekuiper"):
        rates = []
        for rate in grid:
            loss, ratio = DELIVERY_COUNTERS["good" if rate < top else "bad"]
            rates.append(
                {
                    "rate_msg_s": rate,
                    "run_count": 30,
                    "pooled_loss": loss,
                    "mean_achieved_ratio": ratio,
                    "total_duplicates": 0,
                    "classification": "good"
                    if rate < top
                    else ("bad" if system == "mqtt-loopback" else "support-confounded"),
                    "run_summary": {
                        "achieved_rate_msg_s": {
                            "median": rate if rate < top else rate * 0.97,
                            "values": [rate * (0.98 + index / 1_500) for index in range(30)],
                        },
                        "achieved_ratio": {"median": ratio, "values": [ratio] * 30},
                        "loss": {"median": loss, "values": [loss] * 30},
                        "p99_ns": {
                            "median": 100_000 + rate,
                            "values": [90_000 + rate + 1_000 * index for index in range(30)],
                        },
                    },
                    "normalized_p99": {
                        "median": 1.0 if rate < 8_000 else 2.1,
                        "values": [0.9 + 0.05 * index for index in range(30)],
                    },
                }
            )
        systems[system] = {
            "complete": True,
            "delivery_ceiling": {"rate_msg_s": grid[-2], "censoring": "right-censored"},
            "normalized_p99_knee": {"rate_msg_s": 8_000, "censoring": "none"},
            "support_censoring": {
                "from_rate_msg_s": top,
                "highest_support_uncensored_rate_msg_s": grid[-2],
            },
            "rates": rates,
        }
    return {
        "schema_version": 1,
        "experiment": "e-perf-10",
        "thesis_evidence": True,
        "sample_unit": "run",
        "required_runs_per_rate": 30,
        "rate_points_msg_s": list(grid),
        "systems": systems,
    }


def set_capacity_cells(summary: dict, system: str, cells: list[str]) -> None:
    for rate, cell in zip(summary["systems"][system]["rates"], cells, strict=True):
        loss, ratio = DELIVERY_COUNTERS[cell]
        rate["pooled_loss"] = loss
        rate["mean_achieved_ratio"] = ratio
        rate["run_summary"]["loss"]["values"] = [loss] * 30
        rate["run_summary"]["achieved_ratio"]["values"] = [ratio] * 30
        rate["classification"] = cell
    if system == "mqtt-loopback":
        grid = summary["rate_points_msg_s"]
        first_bad = next((rate for rate, cell in zip(grid, cells) if cell == "bad"), None)
        for result in summary["systems"].values():
            result["support_censoring"] = {
                "from_rate_msg_s": first_bad,
                "highest_support_uncensored_rate_msg_s": max(
                    (rate for rate in grid if first_bad is None or rate < first_bad),
                    default=None,
                ),
            }


def capacity_decision(
    wafer: list[str],
    ekuiper: list[str],
    *,
    support: list[str] | None = None,
    grid: tuple[int, ...] = FROZEN_CAPACITY_GRID,
) -> dict:
    summary = capacity_summary(grid)
    set_capacity_cells(summary, "mqtt-loopback", support or ["good"] * len(grid))
    set_capacity_cells(summary, "wafer", wafer)
    set_capacity_cells(summary, "ekuiper", ekuiper)
    return capacity_competitive_decision(summary)


def ceiling(decision: dict, system: str) -> tuple[float, float]:
    bounds = decision["systems"][system]
    return bounds["lower_bound_msg_s"], bounds["upper_bound_msg_s"]


def test_capacity_tables_keep_metrics_and_support_limitation_separate() -> None:
    rates, boundaries = capacity_tables(capacity_summary())
    assert len(rates) == 20
    assert rates["N_runs"].eq(30).all()
    assert {
        "offered_rate_msg_s",
        "median_achieved_rate_msg_s",
        "pooled_loss",
        "total_duplicates",
        "median_p99_ns",
        "median_normalized_p99",
    } <= set(rates.columns)
    assert "achieved_rate_msg_s" not in boundaries.columns
    assert "delivery_ceiling_msg_s" not in boundaries.columns
    assert {
        "delivery_ceiling_lower_bound_msg_s",
        "delivery_ceiling_upper_bound_msg_s",
        "delivery_ceiling_non_monotonic",
        "normalized_p99_knee_msg_s",
        "mqtt_support_path_limitation",
        "wafer_lower_bound_msg_s",
        "wafer_upper_bound_msg_s",
        "ekuiper_lower_bound_msg_s",
        "ekuiper_upper_bound_msg_s",
        "competitive_ratio_lower_bound",
        "competitive_ratio_upper_bound",
        "competitive_threshold",
        "competitive_branch",
        "competitive_status",
        "competitive_reason",
        "support_confounded_rate_msg_s",
        "beyond_grid_limitation",
        "claim_boundary",
    } <= set(boundaries.columns)
    assert boundaries["claim_boundary"].str.contains("support").all()
    by_system = boundaries.set_index("system")
    assert by_system.loc["mqtt-loopback", "delivery_ceiling_lower_bound_msg_s"] == 15_000
    assert by_system.loc["mqtt-loopback", "delivery_ceiling_upper_bound_msg_s"] == 16_000
    assert by_system.loc["native", "delivery_ceiling_upper_bound_msg_s"] == np.inf
    assert boundaries["wafer_lower_bound_msg_s"].eq(15_000).all()
    assert boundaries["wafer_upper_bound_msg_s"].eq(np.inf).all()
    assert boundaries["ekuiper_lower_bound_msg_s"].eq(15_000).all()
    assert boundaries["ekuiper_upper_bound_msg_s"].eq(np.inf).all()
    assert boundaries["competitive_ratio_lower_bound"].eq(0.0).all()
    assert boundaries["competitive_ratio_upper_bound"].eq(np.inf).all()
    assert boundaries["competitive_threshold"].eq(0.70).all()
    assert boundaries["competitive_status"].eq("CENSORED").all()
    assert boundaries["competitive_branch"].eq("straddles-threshold").all()
    assert boundaries["support_confounded_rate_msg_s"].eq(16_000).all()
    assert boundaries["beyond_grid_limitation"].str.contains("16000").all()
    assert set(rates["classification"]) == {"good", "bad", "support-confounded"}


def test_capacity_decision_brackets_each_ceiling_between_tested_rates() -> None:
    both_good = capacity_decision(["good"] * 5, ["good"] * 5)
    assert ceiling(both_good, "wafer") == ceiling(both_good, "ekuiper") == (16_000, np.inf)
    assert (both_good["ratio_lower_bound"], both_good["ratio_upper_bound"]) == (0.0, np.inf)
    assert both_good["status"] == "CENSORED"

    adjacent = capacity_decision(
        ["good", "good", "good", "bad", "bad"],
        ["good", "good", "good", "good", "bad"],
    )
    assert ceiling(adjacent, "wafer") == (8_000, 15_000)
    assert ceiling(adjacent, "ekuiper") == (15_000, 16_000)
    assert (adjacent["ratio_lower_bound"], adjacent["ratio_upper_bound"]) == (0.5, 1.0)
    assert adjacent["branch"] == "straddles-threshold"
    assert adjacent["status"] == "CENSORED"

    passed = capacity_decision(["good"] * 5, ["good", "bad", "bad", "bad", "bad"])
    assert ceiling(passed, "ekuiper") == (1_000, 4_000)
    assert passed["ratio_lower_bound"] == 4.0
    assert passed["branch"] == "worst-case-pass"
    assert passed["status"] == "PASS"

    failed = capacity_decision(
        ["good", "bad", "bad", "bad", "bad"],
        ["good", "good", "good", "good", "bad"],
    )
    assert ceiling(failed, "wafer") == (1_000, 4_000)
    assert failed["ratio_upper_bound"] == pytest.approx(0.267, abs=1e-3)
    assert failed["branch"] == "best-case-fail"
    assert failed["status"] == "FAIL"


def test_capacity_decision_handles_a_failing_lowest_rate() -> None:
    ekuiper_fails = capacity_decision(["good", "good", "bad", "bad", "bad"], ["bad"] * 5)
    assert ceiling(ekuiper_fails, "ekuiper") == (0, 1_000)
    assert ekuiper_fails["ratio_lower_bound"] == 4.0
    assert ekuiper_fails["status"] == "PASS"

    wafer_fails = capacity_decision(["bad"] * 5, ["good", "good", "bad", "bad", "bad"])
    assert ceiling(wafer_fails, "wafer") == (0, 1_000)
    assert wafer_fails["ratio_upper_bound"] == 0.25
    assert wafer_fails["status"] == "FAIL"

    both_fail = capacity_decision(["bad"] * 5, ["bad"] * 5)
    assert (both_fail["ratio_lower_bound"], both_fail["ratio_upper_bound"]) == (0.0, np.inf)
    assert both_fail["status"] == "CENSORED"


def test_capacity_decision_widens_a_non_monotonic_ceiling_over_every_reading() -> None:
    dip = capacity_decision(
        ["good", "bad", "good", "bad", "bad"],
        ["good", "good", "good", "bad", "bad"],
    )
    assert ceiling(dip, "wafer") == (1_000, 15_000)
    assert dip["systems"]["wafer"]["non_monotonic"] is True
    assert dip["systems"]["ekuiper"]["non_monotonic"] is False
    assert dip["status"] == "CENSORED"

    robust = capacity_decision(["good", "good", "bad", "good", "good"], ["bad"] * 5)
    assert ceiling(robust, "wafer") == (4_000, np.inf)
    assert robust["systems"]["wafer"]["non_monotonic"] is True
    assert robust["status"] == "PASS"


def test_capacity_decision_support_confounded_cells_set_no_bound() -> None:
    support = ["good", "good", "bad", "bad", "bad"]
    passed = capacity_decision(
        ["good"] * 5, ["good", "bad", "good", "good", "good"], support=support
    )
    assert ceiling(passed, "wafer") == (4_000, np.inf)
    assert ceiling(passed, "ekuiper") == (1_000, 4_000)
    assert passed["support_confounded_rate_msg_s"] == 8_000
    assert passed["ratio_lower_bound"] == 1.0
    assert passed["status"] == "PASS"

    censored = capacity_decision(["good"] * 5, ["good"] * 5, support=support)
    assert ceiling(censored, "ekuiper") == (4_000, np.inf)
    assert censored["status"] == "CENSORED"


def test_bracket_rates_tighten_both_ceilings_into_a_decision() -> None:
    common = capacity_decision(
        ["good", "good", "bad", "bad", "bad"], ["good", "good", "bad", "bad", "bad"]
    )
    assert (ceiling(common, "wafer"), ceiling(common, "ekuiper")) == ((4_000, 8_000), (4_000, 8_000))
    assert common["status"] == "CENSORED"

    grid = (1_000, 4_000, 4_500, 5_700, 6_300, 8_000, 15_000, 16_000)
    bracketed = capacity_decision(
        ["good", "good", "good", "bad", "bad", "bad", "bad", "bad"],
        ["good", "good", "good", "good", "bad", "bad", "bad", "bad"],
        grid=grid,
    )
    assert ceiling(bracketed, "wafer") == (4_500, 5_700)
    assert ceiling(bracketed, "ekuiper") == (5_700, 6_300)
    assert bracketed["ratio_lower_bound"] == 4_500 / 6_300
    assert bracketed["status"] == "PASS"


def test_capacity_decision_recomputes_delivery_from_the_counters() -> None:
    summary = capacity_summary()
    for system in ("mqtt-loopback", "native", "wafer"):
        set_capacity_cells(summary, system, ["good"] * 5)
    set_capacity_cells(summary, "ekuiper", ["good", "bad", "bad", "bad", "bad"])
    for rate in summary["systems"]["wafer"]["rates"][1:]:
        rate.update(pooled_loss=0.75, mean_achieved_ratio=0.10, total_duplicates=999)
        rate["run_summary"]["loss"]["values"] = [0.75] * 30
        rate["run_summary"]["achieved_ratio"]["values"] = [0.10] * 30
    decision = capacity_competitive_decision(summary)
    assert ceiling(decision, "wafer") == (1_000, 4_000)
    assert decision["status"] == "CENSORED"
    with pytest.raises(ValueError, match="wafer rate 4000 classification disagrees"):
        capacity_tables(summary)

    duplicated = capacity_summary()
    set_capacity_cells(duplicated, "mqtt-loopback", ["good"] * 5)
    set_capacity_cells(duplicated, "wafer", ["good", "good", "good", "bad", "bad"])
    duplicated["systems"]["wafer"]["rates"][2]["total_duplicates"] = 1
    assert ceiling(capacity_competitive_decision(duplicated), "wafer") == (4_000, 8_000)


def test_capacity_decision_accepts_any_sorted_grid() -> None:
    grid = (1_000, 4_000, 6_000, 8_000, 15_000, 16_000)
    decision = capacity_decision(
        ["good", "good", "good", "bad", "bad", "bad"],
        ["good", "good", "bad", "bad", "bad", "bad"],
        grid=grid,
    )
    assert ceiling(decision, "wafer") == (6_000, 8_000)
    assert ceiling(decision, "ekuiper") == (4_000, 6_000)
    assert decision["ratio_lower_bound"] == 1.0
    assert decision["status"] == "PASS"

    rates, boundaries = capacity_tables(capacity_summary(grid))
    assert len(rates) == 24
    assert boundaries["beyond_grid_limitation"].str.contains("16000").all()


def test_capacity_decision_rejects_incomplete_or_malformed_population() -> None:
    incomplete = capacity_summary()
    incomplete["systems"]["wafer"]["rates"][2]["run_count"] = 29
    decision = capacity_competitive_decision(incomplete)
    assert decision["branch"] == "invalid-population"
    assert decision["status"] == "PENDING"
    assert "incomplete tested-rate cell" in decision["reason"]

    support_incomplete = capacity_summary()
    support_incomplete["systems"]["mqtt-loopback"]["rates"][0]["run_count"] = 29
    assert "mqtt-loopback" in capacity_competitive_decision(support_incomplete)["reason"]

    missing_duplicates = capacity_summary()
    del missing_duplicates["systems"]["ekuiper"]["rates"][0]["total_duplicates"]
    assert "malformed delivery counters" in capacity_competitive_decision(
        missing_duplicates
    )["reason"]

    unsorted = capacity_summary()
    unsorted["rate_points_msg_s"] = [4_000, 1_000, 8_000, 15_000, 16_000]
    decision = capacity_competitive_decision(unsorted)
    assert decision["status"] == "PENDING"
    assert "strictly increasing" in decision["reason"]
    assert decision["ratio_lower_bound"] is decision["ratio_upper_bound"] is None


def test_capacity_tables_reject_support_censoring_that_disagrees_with_the_loopback() -> None:
    summary = capacity_summary()
    summary["systems"]["native"]["support_censoring"]["from_rate_msg_s"] = None
    with pytest.raises(ValueError, match="support-path censoring disagrees"):
        capacity_tables(summary)


def test_capacity_tables_reject_pooled_counters_that_disagree_with_their_runs() -> None:
    summary = capacity_summary()
    summary["systems"]["wafer"]["rates"][0]["run_summary"]["loss"]["values"] = [0.5] * 30
    with pytest.raises(ValueError, match="pooled counters disagree"):
        capacity_tables(summary)

    summary = capacity_summary()
    summary["systems"]["ekuiper"]["rates"][1]["mean_achieved_ratio"] = 0.995
    with pytest.raises(ValueError, match="ekuiper rate 4000 pooled counters disagree"):
        capacity_tables(summary)


def swap5_record(run_index: int = 1) -> dict:
    requests = []
    events = []
    for index in range(50):
        timeline = {
            "compile_ns": 1,
            "instantiate_ns": 2,
            "signal_ns": 3,
            "rollback_ns": 4 + index + 100 * run_index,
        }
        requests.append({
            "event_index": index,
            "plugin": "wafer_pass_through_v2_panics.wasm",
            "request_started_ns": 1_000 + index * 100,
            "request_finished_ns": 1_010 + index * 100,
            "http_status": 200,
            "body": {
                "status": "rolled_back",
                "compile_cache": "compiled" if index == 0 else "memory_hit",
                "timeline": timeline,
            },
        })
        events.append({"event_index": index, **timeline})
    sequence = {"expected": 1000, "received": 1000, "gaps": 0, "duplicates": 0}
    rollback = {
        "schema_version": 1,
        "duration_unit": "ns",
        "independent_unit": "complete process run",
        "nested_unit": "rollback event within run",
        "attempts": 50,
        "rolled_back": 50,
        "all_rolled_back": True,
        "sequence": sequence,
        "events": events,
    }
    continuity = {
        "schema_version": 1,
        "clock": "unix-epoch",
        "final_rollback_event_index": 49,
        "final_rollback_finished_ns": requests[-1]["request_finished_ns"],
        "observation_start_ns": requests[-1]["request_finished_ns"],
        "observation_end_ns": requests[-1]["request_finished_ns"] + 100,
        "messages_after_final_rollback": 10,
        "output_observed_after_final_rollback": True,
        "successful_v2_transition_observed": False,
        "interval_metrics_path": "interval-metrics.json",
        "interval_metrics_sha256": "a" * 64,
        "sequence": sequence,
    }
    record = {
        "condition": "process-trap-rollback",
        "run_index": run_index,
        "requests": requests,
        "rollback": rollback,
        "continuity": continuity,
        "sequence": sequence,
    }

    return record


def swap5_records() -> list[dict]:
    return [swap5_record(run_index) for run_index in range(1, 11)]


def test_failed_replacement_table_requires_semantically_valid_rollback() -> None:
    records = swap5_records()
    table = failed_replacement_table(records)
    assert table["run_index"].tolist() == list(range(1, 11))
    assert table["N_nested_events"].eq(50).all()
    assert table["rolled_back_events"].eq(50).all()
    assert table["post_rollback_continuity"].all()

    drifted = json.loads(json.dumps(records))
    drifted[3]["rollback"]["rolled_back"] = 49
    with pytest.raises(ValueError, match="does not reconcile"):
        failed_replacement_table(drifted)


def test_failed_replacement_table_separates_the_first_use_rollback_of_each_run() -> None:
    table = failed_replacement_table(swap5_records()).set_index("run_index")
    assert table.loc[3, "median_first_use_rollback_ns"] == 304
    assert table.loc[3, "median_cached_rollback_ns"] == 329
    assert table.loc[3, "max_rollback_ns"] == 353

    twice_compiled = swap5_records()
    twice_compiled[4]["requests"][1]["body"]["compile_cache"] = "compiled"
    with pytest.raises(ValueError, match="run 5 requires exactly one first-use rollback"):
        failed_replacement_table(twice_compiled)

    late_compile = swap5_records()
    late_compile[1]["requests"][0]["body"]["compile_cache"] = "memory_hit"
    late_compile[1]["requests"][2]["body"]["compile_cache"] = "compiled"
    with pytest.raises(ValueError, match="run 2 requires exactly one first-use rollback event, its first"):
        failed_replacement_table(late_compile)


def test_failed_replacement_table_requires_ten_runs_unless_diagnostic() -> None:
    with pytest.raises(ValueError, match="requires 10 independent runs"):
        failed_replacement_table([swap5_record()])
    with pytest.raises(ValueError, match="requires 10 independent runs"):
        failed_replacement_table([*swap5_records()[:9], swap5_record(11)])
    table = failed_replacement_table([swap5_record(2), swap5_record(1)], canonical=False)
    assert table["run_index"].tolist() == [1, 2]
    assert not table["thesis_evidence"].any()
    assert failed_replacement_table(swap5_records())["thesis_evidence"].all()


def test_failed_replacement_summary_pools_runs_not_rollback_events() -> None:
    summary = failed_replacement_summary(failed_replacement_table(swap5_records())).iloc[0]
    first_use = [4 + 100 * run for run in range(1, 11)]
    cached = [29 + 100 * run for run in range(1, 11)]
    assert summary["N_runs"] == 10
    assert summary["N_nested_events"] == 500
    assert summary["rolled_back_events"] == 500
    assert summary["all_rolled_back"]
    assert summary["median_first_use_rollback_ns"] == np.median(first_use)
    assert (summary["first_use_rollback_ci95_low_ns"], summary["first_use_rollback_ci95_high_ns"]) == bootstrap_ci(np.asarray(first_use, dtype=float))
    assert summary["median_cached_rollback_ns"] == np.median(cached)
    assert (summary["cached_rollback_ci95_low_ns"], summary["cached_rollback_ci95_high_ns"]) == bootstrap_ci(np.asarray(cached, dtype=float))
    assert summary["runs_with_post_rollback_output"] == 10
    assert summary["total_loss"] == 0
    assert summary["total_duplicates"] == 0
    assert summary["thesis_evidence"]


def swap_evidence(events: int = 50, run_index: int = 1) -> dict:
    return {
        "run_index": run_index,
        "events": [
            {
                "event_index": index,
                "compile_cache": "compiled" if index == 0 else "memory_hit",
                "compile_ns": 40_000_000 + run_index if index == 0 else 10_000,
                "instantiate_ns": 200_000,
                "signal_ns": 5_000,
                "replacement_adopted_ns": 100_000 + index,
                "first_post_replacement_local_outcome_ns": 50_000,
                "http_total_ns": 45_000_000 if index == 0 else 500_000,
                "sink_observed_output_gap_ns": 1_000 * run_index if index <= 25 else 1_000_000,
            }
            for index in range(events)
        ],
    }


def swap_runs(events: int = 50) -> list[dict]:
    return [swap_evidence(events, run_index) for run_index in range(1, 11)]


def test_swap_phase_table_summarises_each_run_before_pooling_runs() -> None:
    table = swap_phase_table(swap_runs()).set_index("event_class")
    assert table["N_runs"].eq(10).all()
    assert table.loc["first-use", "N_nested_events"] == 10
    assert table.loc["cached", "N_nested_events"] == 490
    assert table.loc["first-use", "median_compile_ns"] == 40_000_005.5
    assert table.loc["cached", "median_compile_ns"] == 10_000
    assert table.loc["cached", "median_phase_total_ns"] == 10_000 + 200_000 + 5_000 + 100_025 + 50_000
    assert "bootstrap 95% CI over runs" in table.loc["cached", "estimator"]

    run_medians = np.asarray([1_000.0 * run for run in range(1, 11)])
    assert table.loc["cached", "median_sink_observed_output_gap_ns"] == np.median(run_medians) == 5_500
    assert (
        table.loc["cached", "sink_observed_output_gap_ci95_low_ns"],
        table.loc["cached", "sink_observed_output_gap_ci95_high_ns"],
    ) == bootstrap_ci(run_medians)


def test_swap_phase_table_reports_the_run_level_tail_of_cached_swaps() -> None:
    runs = swap_runs()
    for run in runs:
        for event in run["events"][26:]:
            event["sink_observed_output_gap_ns"] = 1_000_000 * run["run_index"]
    table = swap_phase_table(runs).set_index("event_class")
    run_p95 = np.asarray([1_000_000.0 * run for run in range(1, 11)])
    assert table.loc["cached", "median_sink_observed_output_gap_ns"] == 5_500
    assert table.loc["cached", "median_run_p95_sink_observed_output_gap_ns"] == np.median(run_p95)
    assert (
        table.loc["cached", "run_p95_sink_observed_output_gap_ci95_low_ns"],
        table.loc["cached", "run_p95_sink_observed_output_gap_ci95_high_ns"],
    ) == bootstrap_ci(run_p95)
    assert table.loc["cached", "median_run_p95_phase_total_ns"] == pytest.approx(
        10_000 + 200_000 + 5_000 + 100_046.6 + 50_000
    )
    assert pd.isna(table.loc["first-use", "median_run_p95_sink_observed_output_gap_ns"])


def test_swap_phase_table_requires_ten_full_runs_unless_diagnostic() -> None:
    with pytest.raises(ValueError, match="requires 10 independent runs"):
        swap_phase_table([swap_evidence()])
    with pytest.raises(ValueError, match="run 4 requires 50 nested"):
        swap_phase_table([*swap_runs()[:3], swap_evidence(49, run_index=4), *swap_runs()[4:]])
    recompiled = swap_runs()
    recompiled[6]["events"][3]["compile_cache"] = "compiled"
    with pytest.raises(ValueError, match="run 7 requires exactly one first-use"):
        swap_phase_table(recompiled)
    late_compile = swap_runs()
    late_compile[2]["events"][0]["compile_cache"] = "memory_hit"
    late_compile[2]["events"][3]["compile_cache"] = "compiled"
    with pytest.raises(ValueError, match="run 3 requires exactly one first-use swap event, its first"):
        swap_phase_table(late_compile)
    table = swap_phase_table([swap_evidence(3), swap_evidence(3, run_index=2)], canonical=False)
    assert table.set_index("event_class").loc["first-use", "N_runs"] == 2
    assert not table["thesis_evidence"].any()


def test_swap_phase_table_counts_a_stopped_run_among_the_ten() -> None:
    runs = swap_runs()
    runs[3] = {"condition": "wafer", "run_index": 4, "sut_outcome_reasons": ["swap-failed"]}
    table = swap_phase_table(runs).set_index("event_class")
    assert table.loc["first-use", "N_runs"] == 9
    assert table.loc["cached", "N_nested_events"] == 9 * 49
    assert (table.loc["stopped early", "N_runs"], table.loc["stopped early", "N_nested_events"]) == (1, 0)
    assert table["sut_outcome_reasons"].eq("swap-failed").all()


def test_swap_phase_table_rejects_unknown_cache_outcome() -> None:
    runs = swap_runs()
    runs[0]["events"][7]["compile_cache"] = None
    with pytest.raises(ValueError, match="event 7 has unknown compile cache"):
        swap_phase_table(runs)


def test_swap_sequence_table_reports_each_run_and_the_total() -> None:
    records = [
        {"run_index": run, "sequence": {"expected": 120_000, "received": 120_000, "gaps": 0, "duplicates": 0}}
        for run in range(1, 11)
    ]
    table = swap_sequence_table(records)
    assert table["run"].tolist() == [*(str(run) for run in range(1, 11)), "all"]
    total = table.set_index("run").loc["all"]
    assert (total.expected, total.received, total.loss, total.duplicates) == (1_200_000, 1_200_000, 0, 0)
    assert table["lossless"].all()

    records[4]["sequence"] |= {"received": 119_998, "gaps": 2}
    table = swap_sequence_table(records).set_index("run")
    assert not table.loc["5", "lossless"]
    assert table.loc["all", "loss"] == 2
    assert not table.loc["all", "lossless"]
    with pytest.raises(ValueError, match="requires 10 independent runs"):
        swap_sequence_table(records[:9])
    assert not swap_sequence_table(records[:2], canonical=False)["thesis_evidence"].any()


def test_swap_sequence_table_reports_a_stopped_run_as_not_lossless() -> None:
    records = [
        {"run_index": run, "sequence": {"expected": 120_000, "received": 120_000, "gaps": 0, "duplicates": 0}}
        for run in range(1, 11)
    ]
    records[6] = {"run_index": 7, "sequence": None, "sut_outcome_reasons": ["runtime-exit"]}
    table = swap_sequence_table(records).set_index("run")
    assert not table.loc["7", "lossless"]
    assert table.loc["all", "runs_stopped_early"] == 1
    assert (table.loc["all", "expected"], table.loc["all", "loss"]) == (1_080_000, 0)
    assert not table.loc["all", "lossless"]
    assert table.drop(index=["7", "all"])["lossless"].all()


def candidate_scaling_summary(experiment: str, conditions: list[tuple[str, int]]) -> dict:
    records = []
    for condition, value in conditions:
        for run_index in range(1, 6):
            records.append(
                {
                    "schema_version": 1,
                    "batch_class": (
                        "candidate-payload-refinement"
                        if experiment == "e-perf-payload-refinement"
                        else "candidate-depth-extension"
                    ),
                    "experiment": experiment,
                    "evidence_class": "candidate-supplementary",
                    "thesis_evidence": False,
                    "n30_admitted": False,
                    "sample_unit": (
                        "independent host run at one payload size"
                        if experiment == "e-perf-payload-refinement"
                        else "independent host run at one pipeline depth"
                    ),
                    "condition": condition,
                    "run_index": run_index,
                    "source_git_sha": "1" * 40,
                    "source_dirty": False,
                    "payload_bytes" if experiment == "e-perf-payload-refinement" else "depth": value,
                    **(
                        {"payload_sha256": hashlib.sha256(b"B" * value).hexdigest()}
                        if experiment == "e-perf-payload-refinement"
                        else {
                            "node_count": value + 2,
                            "transform_count": value,
                            "edge_count": value + 1,
                            "identical_transform_behavior": True,
                            "effective_metering_mode": "fuel-and-epoch",
                        }
                    ),
                    "latency_ns": {"p50": value * 10, "p95": value * 20, "p99": value * 30},
                    **({"peak_rss_bytes": value * 1024} if experiment == "e-perf-depth-extension" else {}),
                }
            )
    return {
        "schema_version": 1,
        "experiment": experiment,
        "evidence_class": "candidate-supplementary",
        "thesis_evidence": False,
        "n30_admitted": False,
        "sample_unit": records[0]["sample_unit"],
        "required_runs_per_condition": 5,
        "complete": True,
        "no_pool_with": (
            ["e-perf-4", "prior diagnostic rehearsals"]
            if experiment == "e-perf-payload-refinement"
            else ["e-perf-3", "e-perf-6", "e-perf-8", "prior diagnostic rehearsals"]
        ),
        "records": records,
    }


def ekuiper_profile_summary() -> dict:
    records = []
    for rate in (1_000, 4_000, 8_000):
        for run_index in range(1, 6):
            for state in ("profiled", "unprofiled-control"):
                records.append(
                    {
                        "schema_version": 1,
                        "experiment": "e-compare-ekuiper-profile",
                        "evidence_class": "diagnostic",
                        "thesis_evidence": False,
                        "n30_admitted": False,
                        "sample_unit": "independent host run at one rate and profiler state",
                        "condition": f"rate-{rate:05d}/{state}",
                        "run_index": run_index,
                        "rate_msg_s": rate,
                        "profiler_state": state,
                        "source_git_sha": "a" * 40,
                        "source_dirty": False,
                        "measurement_source_leaf": (
                            f"raw/e-compare-ekuiper-profile/rate-{rate:05d}/{state}/"
                            f"run-{run_index:02d}-attempt-01"
                        ),
                        "shared_from": None,
                        "latency_ns": {
                            "sample_count": rate * 60,
                            "p50": 100_000,
                            "p95": 220_000 if state == "profiled" else 200_000,
                            "p99": 330_000 if state == "profiled" else 300_000,
                        },
                        "interval_alignment": {
                            "clock": "unix-epoch",
                            "measurement_start_ns": 10_000_000_000,
                            "measurement_end_ns": 70_000_000_000,
                            "row_count": 60,
                            "maximum_rows": 73,
                            "path": "interval-metrics.json",
                            "sha256": "b" * 64,
                        },
                        "process_metrics": (
                            {
                                "status": "available",
                                "row_count": 61,
                                "maximum_rows": 62,
                                "cpu_percent": 42.0,
                                "max_rss_bytes": 32_000_000,
                            }
                            if state == "profiled"
                            else {
                                "status": "unavailable",
                                "reason": "process-profiler-disabled-by-design",
                            }
                        ),
                        "gc_runtime_metrics": (
                            {
                                "status": "available",
                                "source": "go-gctrace-journal",
                                "path": "ekuiper-gctrace.log",
                                "sha256": "c" * 64,
                                "trace_line_count": 90,
                                "missing_cycle_count": 0,
                                "cycle_count": 60,
                                "stw_pause_total_ns": 6_000_000,
                                "stw_pause_max_ns": 400_000,
                                "max_heap_at_start_mib": 12,
                                "max_live_heap_mib": 6,
                                "max_heap_goal_mib": 12,
                            }
                            if state == "profiled"
                            else {
                                "status": "unavailable",
                                "reason": "gctrace-disabled-by-design",
                            }
                        ),
                        "claim_boundary": "diagnostic-association-only-not-gc-causality",
                        "profiler_overhead": {
                            "experiment": "e-compare-ekuiper-profile",
                            "condition": f"rate-{rate:05d}/{state}",
                            "run_index": run_index,
                            "rate_msg_s": rate,
                            "profiler_state": state,
                            "paired_condition": (
                                f"rate-{rate:05d}/"
                                f"{'unprofiled-control' if state == 'profiled' else 'profiled'}"
                            ),
                            "pair_key": f"rate-{rate:05d}/run-{run_index:02d}",
                            "overhead_estimator": (
                                "paired-run-level-profiled-minus-unprofiled-control"
                            ),
                            "claim_boundary": (
                                "diagnostic-association-only-not-gc-causality"
                            ),
                        },
                        "no_pool_with": [
                            "e-perf-1",
                            "e-perf-10",
                            "prior diagnostic rehearsals",
                        ],
                    }
                )
    return {
        "schema_version": 1,
        "experiment": "e-compare-ekuiper-profile",
        "batch_id": "fixture",
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
        "records": records,
    }


def test_ekuiper_profile_tables_keep_thirty_runs_and_fifteen_pairs() -> None:
    runs, pairs = ekuiper_profile_tables(ekuiper_profile_summary())

    assert len(runs) == 30
    assert len(pairs) == 15
    assert runs.groupby(["rate_msg_s", "profiler_state"]).size().eq(5).all()
    assert pairs.groupby("rate_msg_s").size().eq(5).all()
    assert pairs["p95_overhead_ns"].eq(20_000).all()
    assert pairs["p99_overhead_ns"].eq(30_000).all()
    assert pairs["interpretation"].eq(
        "diagnostic-association-only-not-gc-causality"
    ).all()
    assert runs.loc[runs.profiler_state == "unprofiled-control", "process_status"].eq(
        "unavailable"
    ).all()
    profiled = runs[runs.profiler_state == "profiled"]
    assert profiled["gc_status"].eq("available").all()
    assert profiled["gc_cycle_count"].eq(60).all()
    assert profiled["gc_stw_pause_max_ns"].eq(400_000).all()
    assert runs.loc[runs.profiler_state == "unprofiled-control", "gc_status"].eq(
        "unavailable"
    ).all()


def test_ekuiper_profile_tables_reject_gc_traces_in_the_unprofiled_control() -> None:
    summary = ekuiper_profile_summary()
    control = next(
        record
        for record in summary["records"]
        if record["profiler_state"] == "unprofiled-control"
    )
    control["gc_runtime_metrics"] = {**summary["records"][0]["gc_runtime_metrics"]}

    with pytest.raises(ValueError, match="GC trace evidence"):
        ekuiper_profile_tables(summary)


def test_canonical_ekuiper_profile_remains_single_release_only() -> None:
    summary = ekuiper_profile_summary()
    summary["records"][1]["source_git_sha"] = "b" * 40

    with pytest.raises(ValueError, match="mixes source revisions"):
        ekuiper_profile_tables(summary)


def test_ekuiper_profile_tables_accept_rows_up_to_the_declared_bound() -> None:
    for row_count in (1, 58, 61, 65, 73):
        summary = ekuiper_profile_summary()
        summary["records"][0]["interval_alignment"]["row_count"] = row_count

        runs, pairs = ekuiper_profile_tables(summary)

        assert len(runs) == 30
        assert len(pairs) == 15

    summary["records"][0]["interval_alignment"]["row_count"] = 74
    with pytest.raises(ValueError, match="interval alignment"):
        ekuiper_profile_tables(summary)


def test_ekuiper_profile_tables_reject_missing_pairs_aliases_and_causal_claims() -> None:
    summary = ekuiper_profile_summary()
    summary["records"].pop()
    with pytest.raises(ValueError, match="matched run grid"):
        ekuiper_profile_tables(summary)

    summary = ekuiper_profile_summary()
    summary["records"][0]["shared_from"] = "e-perf-1"
    with pytest.raises(ValueError, match="diagnostic boundary"):
        ekuiper_profile_tables(summary)

    summary = ekuiper_profile_summary()
    summary["records"][0]["profiler_overhead"]["paired_condition"] = (
        "rate-01000/profiled"
    )
    with pytest.raises(ValueError, match="pairing evidence"):
        ekuiper_profile_tables(summary)

    summary = ekuiper_profile_summary()
    summary["records"][0]["interval_alignment"]["row_count"] = 0
    with pytest.raises(ValueError, match="interval alignment"):
        ekuiper_profile_tables(summary)

    summary = ekuiper_profile_summary()
    summary["claim_boundary"] = "GC caused latency tails"
    with pytest.raises(ValueError, match="diagnostic identity"):
        ekuiper_profile_tables(summary)


def test_candidate_payload_table_keeps_all_fifty_runs_independent() -> None:
    summary = candidate_scaling_summary(
        "e-perf-payload-refinement",
        [(label, size) for label, size in (
            ("120b", 120), ("1kb", 1024), ("8kb", 8192), ("10kb", 10240),
            ("16kb", 16384), ("32kb", 32768), ("64kb", 65536),
            ("100kb", 102400), ("128kb", 131072), ("256kb", 262144),
        )],
    )
    table = candidate_payload_table(summary)

    assert len(table) == 50
    assert table.groupby("payload_bytes").size().eq(5).all()
    assert table["run_index"].isin(range(1, 6)).all()
    assert table["thesis_evidence"].eq(False).all()
    assert table["n30_admitted"].eq(False).all()


def test_candidate_depth_table_keeps_latency_and_rss_on_same_thirty_runs() -> None:
    summary = candidate_scaling_summary(
        "e-perf-depth-extension",
        [(f"depth-{depth}", depth) for depth in (1, 3, 5, 10, 20, 50)],
    )
    table = candidate_depth_table(summary)

    assert len(table) == 30
    assert table.groupby("depth").size().eq(5).all()
    assert table["peak_rss_bytes"].notna().all()
    assert table["p95_ns"].notna().all()
    assert table["thesis_evidence"].eq(False).all()


def test_candidate_scaling_tables_reject_primary_or_rehearsal_pooling() -> None:
    summary = candidate_scaling_summary(
        "e-perf-payload-refinement", [("120b", 120)]
    )
    summary["records"][0]["experiment"] = "e-perf-4"
    with pytest.raises(ValueError, match="candidate identity"):
        candidate_payload_table(summary)

    summary = candidate_scaling_summary(
        "e-perf-depth-extension", [("depth-1", 1)]
    )
    summary["records"][0]["batch_class"] = "diagnostic-rehearsal"
    with pytest.raises(ValueError, match="candidate identity"):
        candidate_depth_table(summary)


def candidate_swap_summary(experiment: str) -> dict:
    rollback = experiment == "e-swap-rollback-sessions"
    metrics = (
        {
            "compile_ns": 100,
            "instantiate_ns": 20,
            "signal_ns": 3,
            "rollback_ns": 5,
            "http_total_ns": 150,
        }
        if rollback
        else {
            "compile_ns": 100,
            "instantiate_ns": 20,
            "signal_ns": 3,
            "replacement_adopted_ns": 4,
            "first_post_replacement_local_outcome_ns": 5,
            "http_total_ns": 150,
            "sink_observed_output_gap_ns": 1_000_000,
        }
    )
    records = []
    for run_index in range(1, 6):
        events = [
            {
                "event_index": event_index,
                "event_class": "first-use-aot" if event_index == 0 else "cached",
                "plugin": (
                    "wafer_pass_through_v2_panics.wasm"
                    if rollback
                    else "wafer_pass_through_v2.wasm"
                    if event_index % 2 == 0
                    else "wafer_pass_through_v1.wasm"
                ),
                **metrics,
            }
            for event_index in range(50)
        ]
        records.append(
            {
                "schema_version": 1,
                "batch_class": (
                    "candidate-rollback-session"
                    if rollback
                    else "candidate-independent-swap"
                ),
                "experiment": experiment,
                "condition": "process-trap-rollback" if rollback else "steady",
                "evidence_class": "candidate-supplementary",
                "thesis_evidence": False,
                "n30_admitted": False,
                "sample_unit": "independent host run",
                "nested_unit": (
                    "rollback event within run" if rollback else "swap event within run"
                ),
                "duration_unit": "ns",
                "sample_count": 50,
                "run_index": run_index,
                "source_git_sha": "1" * 40,
                "source_dirty": False,
                "event_classes": ["first-use-aot", "cached"],
                "shared_from": None,
                "sequence": {
                    "expected": 1_000,
                    "received": 1_000,
                    "gaps": 0,
                    "duplicates": 0,
                },
                "no_pool_with": (
                    ["e-swap-5", "prior diagnostic rehearsals"]
                    if rollback
                    else [
                        "e-swap-1",
                        "e-swap-2",
                        "e-swap-6",
                        "prior diagnostic rehearsals",
                    ]
                ),
                "events": events,
                **(
                    {"attempts": 50, "rolled_back": 50, "all_rolled_back": True}
                    if rollback
                    else {}
                ),
            }
        )
    return {
        "schema_version": 1,
        "experiment": experiment,
        "evidence_class": "candidate-supplementary",
        "thesis_evidence": False,
        "n30_admitted": False,
        "sample_unit": "independent host run",
        "nested_unit": "rollback event within run" if rollback else "swap event within run",
        "required_runs": 5,
        "events_per_run": 50,
        "event_classes": ["first-use-aot", "cached"],
        "complete": True,
        "no_pool_with": records[0]["no_pool_with"],
        "records": records,
    }


def test_candidate_swap_tables_keep_events_nested_and_classes_separate() -> None:
    events, runs = candidate_swap_tables(
        candidate_swap_summary("e-swap-independent-sessions")
    )

    assert len(events) == 250
    assert len(runs) == 10
    assert set(runs["event_class"]) == {"first-use-aot", "cached"}
    assert runs.loc[runs.event_class == "first-use-aot", "event_count"].eq(1).all()
    assert runs.loc[runs.event_class == "cached", "event_count"].eq(49).all()
    assert events.groupby("run_index").size().eq(50).all()


def test_candidate_rollback_tables_preserve_five_run_level_populations() -> None:
    events, runs = candidate_swap_tables(
        candidate_swap_summary("e-swap-rollback-sessions")
    )

    assert len(events) == 250
    assert len(runs) == 10
    assert "median_rollback_ns" in runs
    assert events.groupby("run_index").size().eq(50).all()


def test_candidate_rollback_tables_reject_incomplete_rollback_population() -> None:
    summary = candidate_swap_summary("e-swap-rollback-sessions")
    summary["records"][0]["rolled_back"] = 49
    with pytest.raises(ValueError, match="fifty successful rollbacks"):
        candidate_swap_tables(summary)


def test_candidate_swap_tables_reject_mixed_classes_aliases_and_duplicate_runs() -> None:
    summary = candidate_swap_summary("e-swap-independent-sessions")
    summary["records"][0]["events"][0]["event_class"] = "cached"
    with pytest.raises(ValueError, match="classes, indices, or plugins are mixed"):
        candidate_swap_tables(summary)

    summary = candidate_swap_summary("e-swap-independent-sessions")
    summary["records"][0]["shared_from"] = "e-swap-1"
    with pytest.raises(ValueError, match="independent-run identity"):
        candidate_swap_tables(summary)

    summary = candidate_swap_summary("e-swap-independent-sessions")
    summary["records"][1]["run_index"] = 1
    with pytest.raises(ValueError, match="independent-run identity"):
        candidate_swap_tables(summary)


def candidate_capacity_summary() -> dict:
    grids = {
        "mqtt-loopback": [*range(4_000, 16_000, 1_000), 15_250, 15_500, 15_750, 16_000],
        "native": list(range(8_000, 16_000, 1_000)),
        "wafer": list(range(8_000, 16_000, 1_000)),
        "ekuiper": list(range(4_000, 9_000, 1_000)),
    }
    systems = {}
    for system, rates in grids.items():
        systems[system] = {
            "complete": True,
            "support_censoring": {
                "from_rate_msg_s": 8_000,
                "highest_support_uncensored_rate_msg_s": max(
                    (rate for rate in rates if rate < 8_000), default=None
                ),
            },
            "rates": [
                {
                    "rate_msg_s": rate,
                    "run_count": 5,
                    "classification": (
                        "bad"
                        if system == "mqtt-loopback" and rate >= 8_000
                        else "support-confounded"
                        if system != "mqtt-loopback" and rate >= 8_000
                        else "good"
                    ),
                    "pooled_loss": 0.02 if system == "mqtt-loopback" and rate >= 8_000 else 0.0,
                    "mean_achieved_ratio": 0.98 if system == "mqtt-loopback" and rate >= 8_000 else 1.0,
                    "duplicates": 0,
                }
                for rate in rates
            ],
        }
    return {
        "schema_version": 1,
        "experiment": "e-perf-capacity-knee",
        "evidence_class": "candidate-supplementary",
        "thesis_evidence": False,
        "n30_admitted": False,
        "sample_unit": "independent host run at one system and offered rate",
        "required_runs_per_rate": 5,
        "criteria": {
            "loss_aggregation": "sum(total_undelivered) / sum(intended)",
            "max_pooled_loss": 0.01,
            "achieved_aggregation": "mean(run achieved_rate / intended_rate)",
            "min_mean_achieved_ratio": 0.99,
            "duplicates_allowed": 0,
            "support_path_censoring": "mqtt-loopback",
        },
        "systems": systems,
    }


def test_candidate_capacity_table_preserves_estimator_and_censoring() -> None:
    table = candidate_capacity_table(candidate_capacity_summary())

    assert len(table) == 37
    assert table["N_runs"].eq(5).all()
    assert table["thesis_evidence"].eq(False).all()
    assert table["n30_admitted"].eq(False).all()
    assert table.loc[
        (table.system == "wafer") & (table.offered_rate_msg_s == 8_000),
        "classification",
    ].item() == "support-confounded"


def test_candidate_capacity_table_rejects_uncensored_or_threshold_shifted_summary() -> None:
    summary = candidate_capacity_summary()
    summary["systems"]["wafer"]["rates"][0]["classification"] = "good"
    with pytest.raises(ValueError, match="classification disagrees"):
        candidate_capacity_table(summary)

    summary = candidate_capacity_summary()
    summary["criteria"]["max_pooled_loss"] = 0.02
    with pytest.raises(ValueError, match="criteria differ"):
        candidate_capacity_table(summary)

    summary = candidate_capacity_summary()
    summary["systems"]["wafer"]["support_censoring"]["from_rate_msg_s"] = 9_000
    with pytest.raises(ValueError, match="support-path censoring is invalid"):
        candidate_capacity_table(summary)


def test_capacity_tables_reject_schema_drift_from_the_producer() -> None:
    summary = capacity_summary()
    del summary["systems"]["wafer"]["rates"][0]["run_summary"]
    with pytest.raises(TypeError, match="rate summary is malformed"):
        capacity_tables(summary)

    summary = capacity_summary()
    summary["rate_points_msg_s"][-1] = 32_000
    with pytest.raises(ValueError, match="differ from tested grid"):
        capacity_tables(summary)

    summary = capacity_summary((4_000, 8_000, 15_000, 16_000))
    with pytest.raises(ValueError, match="1,000 msg/s baseline"):
        capacity_tables(summary)


def test_scout_diagnostic_arm_never_enters_the_capacity_decision() -> None:
    summary = capacity_summary()
    expected = capacity_competitive_decision(summary)
    summary["systems"]["wafer-max-inflight-1"] = json.loads(
        json.dumps(summary["systems"]["wafer"])
    )
    set_capacity_cells(summary, "wafer-max-inflight-1", ["bad"] * 5)
    assert capacity_competitive_decision(summary) == expected
    with pytest.raises(ValueError, match="requires all four systems"):
        capacity_tables(summary)

    candidate = candidate_capacity_summary()
    candidate["systems"]["wafer-max-inflight-1"] = candidate["systems"]["wafer"]
    with pytest.raises(ValueError, match="requires all four systems"):
        candidate_capacity_table(candidate)


def test_capacity_intervals_bootstrap_the_run_values() -> None:
    summary = capacity_summary()
    rates, _ = capacity_tables(summary)
    wafer = rates[rates["system"] == "wafer"].set_index("offered_rate_msg_s")
    baseline = np.asarray(
        summary["systems"]["wafer"]["rates"][0]["run_summary"]["p99_ns"]["values"], dtype=float
    )
    row = wafer.loc[1_000]
    assert (row["p99_ci95_low_ns"], row["p99_ci95_high_ns"]) == bootstrap_ci(baseline)
    assert [row[f"{stat}_p99_ns"] for stat in ("min", "q1", "median", "q3", "max")] == [
        baseline.min(),
        np.quantile(baseline, 0.25),
        np.median(baseline),
        np.quantile(baseline, 0.75),
        baseline.max(),
    ]
    assert row["total_duplicates"] == 0
    columns = list(rates.columns)
    for metric, low, high in (
        ("achieved_rate_msg_s", "achieved_ci95_low_msg_s", "achieved_ci95_high_msg_s"),
        ("achieved_ratio", "achieved_ratio_ci95_low", "achieved_ratio_ci95_high"),
        ("loss", "loss_ci95_low", "loss_ci95_high"),
        ("p99_ns", "p99_ci95_low_ns", "p99_ci95_high_ns"),
    ):
        start = columns.index(f"median_{metric}")
        assert columns[start : start + 7] == [
            f"median_{metric}",
            low,
            high,
            f"min_{metric}",
            f"q1_{metric}",
            f"q3_{metric}",
            f"max_{metric}",
        ]
    assert "pooled_loss_ci95_high" in columns

    loaded = np.asarray(
        summary["systems"]["wafer"]["rates"][2]["run_summary"]["p99_ns"]["values"], dtype=float
    )
    _, low, high = median_shift_ci(loaded, baseline, relative=True)
    assert wafer.loc[8_000, "median_normalized_p99"] == np.median(loaded) / np.median(baseline)
    assert (
        wafer.loc[8_000, "normalized_p99_ci95_low"],
        wafer.loc[8_000, "normalized_p99_ci95_high"],
    ) == (1 + low, 1 + high)

    summary["systems"]["wafer"]["rates"][0]["run_summary"]["p99_ns"]["values"].pop()
    with pytest.raises(ValueError, match="30 run values"):
        capacity_tables(summary)


def swap3_runs() -> list[dict]:
    return [
        {
            "run_index": run,
            "strategy": strategy,
            "baseline_rate_msg_s": 1_000,
            "event_min_rate_msg_s": 980,
            "dip_percent": 2.0,
            "placebo_dip_percent": 1.0,
            "interruption_ns": 100_000_000,
            "recovery_ns": 200_000_000,
            "recovery_right_censored": False,
            "action_duration_ns": 10_000_000,
            "loss": 0,
            "duplicates": 0,
        }
        for strategy in (
            "wafer-hotswap",
            "wafer-restart",
            "ekuiper-rule-update",
            "ekuiper-make-before-break",
        )
        for run in range(1, 31)
    ]


def test_swap3_table_separates_event_metrics_and_applies_hot_swap_threshold() -> None:
    table = swap3_table(swap3_runs())
    assert table["N_runs"].eq(30).all()
    assert {
        "median_dip_percent",
        "median_interruption_ns",
        "median_action_duration_ns",
        "median_recovery_ns",
        "total_loss",
        "total_duplicates",
    } <= set(table.columns)
    wafer = table.loc[table.strategy == "wafer-hotswap"].iloc[0]
    assert (
        wafer["threshold"]
        == "one-sided 95% upper bound of the median dip < 5 percent; zero loss; zero duplication"
    )
    assert wafer["dip_ci95_high_percent"] < 5
    assert wafer["verdict"] == "PASS"


def test_swap3_table_reports_the_placebo_dip_of_every_arm_without_judging_it() -> None:
    runs = swap3_runs()
    for run in runs:
        if run["strategy"] == "ekuiper-rule-update":
            run["placebo_dip_percent"] = 40.0 + run["run_index"] % 3
    table = swap3_table(runs).set_index("strategy")
    assert table["median_placebo_dip_percent"].to_dict() == {
        "wafer-hotswap": 1.0,
        "wafer-restart": 1.0,
        "ekuiper-rule-update": 41.0,
        "ekuiper-make-before-break": 1.0,
    }
    update = table.loc["ekuiper-rule-update"]
    assert update["placebo_dip_ci95_low_percent"] <= 41.0 <= update["placebo_dip_ci95_high_percent"]

    noisy = [dict(run, placebo_dip_percent=90.0) for run in swap3_runs()]
    assert swap3_table(noisy).set_index("strategy").loc["wafer-hotswap", "verdict"] == "PASS"


def swap4_runs() -> list[dict]:
    return [
        {
            "run_index": run,
            "successful_swaps": 1,
            "sink_observed_output_gap_ns": run * 1_000_000,
            "loss": 0,
            "sequence": {"duplicates": 0},
            "primary_received_events": 129_999 if run % 2 else 130_000,
            "drain_received_events": 1 if run % 2 else 0,
            "drain_first_offset_ns": 120_000_000_000 + run if run % 2 else None,
            "drain_last_offset_ns": 120_000_000_000 + run if run % 2 else None,
            "drain_duration_after_window_ns": run if run % 2 else 0,
            "max_arrival_offset_ns": (
                120_000_000_000 + run if run % 2 else 119_999_000_000
            ),
            "drain_right_censored": False,
            "internal_swap_phases_ns": {
                "compile_ns": 1,
                "instantiate_ns": 2,
                "signal_ns": 3,
                "replacement_adopted_ns": 4,
                "first_post_replacement_local_outcome_ns": 5,
            },
        }
        for run in range(1, 31)
    ]


def test_swap4_table_uses_one_event_per_run_and_reports_p95() -> None:
    table = swap4_table(swap4_runs())
    assert table.loc[0, "N_runs"] == 30
    assert table.loc[0, "N_events"] == 30
    assert table.loc[0, "p95_sink_gap_ns"] == 29_000_000
    assert table.loc[0, "median_compile_ns"] == 1
    assert table.loc[0, "median_first_post_replacement_local_outcome_ns"] == 5
    assert table.loc[0, "runs_with_drain_arrivals"] == 15
    assert table.loc[0, "max_drain_arrival_offset_ns"] == 120_000_000_029
    assert table.loc[0, "drain_right_censored_runs"] == 0
    assert (
        table.loc[0, "threshold"]
        == "one-sided 95% upper bound of the across-run p95 sink gap < 100 ms; zero full-run loss; zero duplication; no receive at or after 130 s"
    )
    broken = swap4_runs()
    broken[0]["successful_swaps"] = 2
    with pytest.raises(ValueError, match="one successful swap"):
        swap4_table(broken)
    broken = swap4_runs()
    broken[0]["run_index"] = 30
    with pytest.raises(ValueError, match="30 independent runs"):
        swap4_table(broken)


def test_bootstrap_and_effect_sizes_reject_empty_inputs() -> None:
    with pytest.raises(ValueError, match="non-empty"):
        bootstrap_ci(np.array([]))
    with pytest.raises(ValueError, match="non-empty"):
        cliffs_delta(np.array([]), np.array([1.0]))


def hash_tree(root: Path) -> dict[str, str]:
    return {
        str(path.relative_to(root)): hashlib.sha256(path.read_bytes()).hexdigest()
        for path in sorted(root.rglob("*"))
        if path.is_file()
    }


def test_analysis_consumers_do_not_modify_raw_artifacts(tmp_path: Path) -> None:
    raw = tmp_path / "raw"
    raw.mkdir()
    (raw / "capacity-summary.json").write_text(json.dumps(capacity_summary()))
    (raw / "swap3.json").write_text(json.dumps(swap3_runs()))
    (raw / "swap4.json").write_text(json.dumps(swap4_runs()))
    before = hash_tree(raw)
    capacity_tables(json.loads((raw / "capacity-summary.json").read_text()))
    swap3_table(json.loads((raw / "swap3.json").read_text()))
    swap4_table(json.loads((raw / "swap4.json").read_text()))
    assert hash_tree(raw) == before


ATTACKS = {
    "e-iso-1": ("buffer-overflow", "traps_memory_out_of_bounds"),
    "e-iso-2": ("cross-read", "traps_memory_out_of_bounds"),
    "e-iso-3": ("fs-access", "guest_unrecoverable"),
    "e-iso-4": ("infinite-loop", "traps_interrupt"),
    "e-iso-5": ("memory-exhaust", "traps_memory_limit"),
    "e-iso-6": ("panic", "traps_unreachable"),
}


def containment_records(runs: int = 30, escaped: dict[str, int] | None = None) -> list[dict]:
    escaped = escaped or {}
    return [
        {
            "experiment": experiment,
            "condition": condition,
            "run_index": run,
            "contained": run > escaped.get(experiment, 0),
            "expected_mechanism": mechanism,
            "expected_count": 3,
            "unexpected_outcomes": 0 if run > escaped.get(experiment, 0) else 1,
            "runtime_panic": False,
            "healthy_messages_out": 60_000,
        }
        for experiment, (condition, mechanism) in ATTACKS.items()
        for run in range(1, runs + 1)
    ]


def test_containment_table_bounds_the_escape_probability_per_attack() -> None:
    table = containment_table(containment_records())
    assert table["experiment"].tolist() == list(ATTACKS)
    assert table["contained_runs"].eq(30).all() and table["all_contained"].all()
    assert table["containment_ci95_low"].iloc[0] == pytest.approx(0.8843, abs=1e-4)
    assert table["escape_probability_upper975"].iloc[0] == pytest.approx(0.1157, abs=1e-4)
    assert table["thesis_evidence"].all()
    assert (table["threshold"] == "every run contained").all()


def test_one_escaped_run_fails_only_its_attack() -> None:
    table = containment_table(containment_records(escaped={"e-iso-5": 1})).set_index("experiment")
    assert table.loc["e-iso-5", "contained_runs"] == 29
    assert not table.loc["e-iso-5", "all_contained"]
    assert table.loc["e-iso-5", "max_unexpected_outcomes"] == 1
    assert table.drop("e-iso-5")["all_contained"].all()


def test_containment_table_requires_every_attack_at_full_n() -> None:
    records = [record for record in containment_records() if record["experiment"] != "e-iso-3"]
    with pytest.raises(ValueError, match="30 independent runs"):
        containment_table(records)
    summary = containment_table(records, canonical=False)
    assert "e-iso-3" not in set(summary.experiment)
    assert not summary["thesis_evidence"].any()


def test_containment_table_rejects_a_run_without_a_verdict() -> None:
    records = containment_records()
    records[0]["contained"] = None
    with pytest.raises(ValueError, match="without a containment verdict"):
        containment_table(records)


def test_containment_summary_rejects_duplicate_runs_in_any_mode() -> None:
    records = containment_records(runs=2)
    records[1]["run_index"] = 1
    with pytest.raises(ValueError, match="duplicate run identity"):
        containment_table(records, canonical=False)


def branch_records(throughput: dict[str, float], runs: int = 30) -> list[dict]:
    return [
        {
            "condition": condition,
            "run_index": run,
            "branches": {
                "branch_a": {
                    "offered_messages": 60_000,
                    "lost_messages": 0,
                    "throughput": {"mean_messages_per_second": rate + run % 3},
                    "latency_ns": {"p95": 120_000 + run},
                }
            },
        }
        for condition, rate in throughput.items()
        for run in range(1, runs + 1)
    ]


def test_branch_isolation_table_contrasts_each_attack_with_the_control() -> None:
    table = branch_isolation_table(
        branch_records({"control": 1_000, "panic-attack": 995, "epoch-loop-attack": 900})
    ).set_index("condition")
    assert table.loc["panic-attack", "throughput_drop_percent"] == pytest.approx(100 * 5 / 1_001)
    assert table.loc["panic-attack", "verdict"] == "PASS"
    assert table.loc["epoch-loop-attack", "throughput_drop_percent"] == pytest.approx(100 * 100 / 1_001)
    assert table.loc["epoch-loop-attack", "verdict"] == "FAIL"
    low, high = table.loc["epoch-loop-attack", ["drop_ci95_low_percent", "drop_ci95_high_percent"]]
    assert low <= 100 * 100 / 1_001 <= high
    assert table.loc["epoch-loop-attack", "throughput_cliffs_delta"] == -1.0
    assert pd.isna(table.loc["control", "throughput_drop_percent"])
    assert table["pooled_loss"].eq(0).all() and table["thesis_evidence"].all()


def test_branch_isolation_table_requires_full_n_unless_diagnostic() -> None:
    records = branch_records({"control": 1_000, "panic-attack": 995, "epoch-loop-attack": 995}, runs=2)
    with pytest.raises(ValueError, match="30 independent runs"):
        branch_isolation_table(records)
    assert not branch_isolation_table(records, canonical=False)["thesis_evidence"].any()
    records[1]["run_index"] = 1
    with pytest.raises(ValueError, match="duplicate run identity"):
        branch_isolation_table(records, canonical=False)


def recovery_records(runs: int = 30) -> list[dict]:
    return [
        {"condition": "panic-recovery", "run_index": run, "durations_ns": [100_000 * run, 200_000 * run, 300_000 * run]}
        for run in range(1, runs + 1)
    ]


def test_recovery_table_summarises_runs_before_pooling() -> None:
    table = recovery_table(recovery_records())
    row = table.iloc[0]
    assert row["N_runs"] == 30 and row["recovery_samples"] == 90
    assert row["median_run_median_ns"] == pytest.approx(3_100_000)
    assert row["median_ci95_low_ns"] <= row["median_run_median_ns"] <= row["median_ci95_high_ns"]
    assert row["pooled_max_ns"] == 9_000_000
    assert "not independent" in row["estimator"]


def test_recovery_table_rejects_a_run_without_samples() -> None:
    records = recovery_records()
    records[4]["durations_ns"] = []
    with pytest.raises(ValueError, match="at least one recovery sample"):
        recovery_table(records)
    with pytest.raises(ValueError, match="30 independent runs"):
        recovery_table(recovery_records(runs=29))


def depth_records(runs: int = 30, depths: tuple[int, ...] = (1, 3, 5, 10)) -> list[dict]:
    return [
        {"condition": f"depth-{depth}", "run_index": run, "p50_ns": 40_000 + 7_500 * depth + run % 5}
        for depth in depths
        for run in range(1, runs + 1)
    ]


def test_depth_tables_report_medians_and_the_per_transform_slope() -> None:
    per_depth, fit = depth_tables(depth_records(), "p50_ns", units="nanoseconds")
    assert per_depth["depth"].tolist() == [1, 3, 5, 10]
    assert per_depth["median_p50_ns"].tolist() == pytest.approx([47_502, 62_502, 77_502, 115_002])
    row = fit.iloc[0]
    assert row["slope_per_depth"] == pytest.approx(7_500, abs=0.5)
    assert row["slope_ci95_low"] <= row["slope_per_depth"] <= row["slope_ci95_high"]
    assert row["N_runs"] == 120 and row["r_squared"] > 0.99
    assert row["units"] == "nanoseconds per added transform"
    assert "not a decision rule" in row["claim_boundary"]


def test_depth_tables_require_every_depth_at_full_n_unless_diagnostic() -> None:
    with pytest.raises(ValueError, match="30 independent runs"):
        depth_tables(depth_records(depths=(1, 3, 5)), "p50_ns", units="nanoseconds")
    per_depth, fit = depth_tables(depth_records(runs=2, depths=(1, 3)), "p50_ns", units="nanoseconds", canonical=False)
    assert len(per_depth) == 2 and not fit["thesis_evidence"].any()
    per_depth, fit = depth_tables(depth_records(runs=2, depths=(1,)), "p50_ns", units="nanoseconds", canonical=False)
    assert len(per_depth) == 1 and fit.empty


BOUNDARY_NS = {"120b": 10_000, "1kb": 10_500, "10kb": 15_000, "100kb": 60_000}


def payload_records(runs: int = 30, sizes: tuple[str, ...] = tuple(BOUNDARY_NS)) -> list[dict]:
    records = []
    for size in sizes:
        for run in range(1, runs + 1):
            native = 20_000 + 1_000 * run
            wafer = native + BOUNDARY_NS[size] + 2_000 * (run % 2)
            for condition, p50 in ((size, wafer), (f"native-{size}", native)):
                records.append(
                    {
                        "condition": condition,
                        "run_index": run,
                        "service_p50_ns": p50,
                        "service_p95_ns": 2 * p50,
                        "service_p99_ns": 3 * p50,
                        "total_expected": 60_000,
                        "received_unique": 60_000 - (run == 1),
                        "duplicates": 0,
                    }
                )
    return records


def test_payload_table_pairs_wafer_and_native_runs_by_run_index() -> None:
    records = payload_records()
    random.Random(7).shuffle(records)
    table = payload_table(records).set_index("condition")
    assert table["payload_bytes"].tolist() == [120, 1_024, 10_240, 102_400]
    assert table["N_pairs"].eq(30).all()
    assert table.loc["10kb", "boundary_p50_ns"] == pytest.approx(16_000)
    assert 15_000 <= table.loc["10kb", "boundary_p50_ci95_low_ns"]
    assert table.loc["10kb", "boundary_p50_ci95_high_ns"] <= 17_000
    assert table.loc["10kb", "native_median_service_p50_ns"] == pytest.approx(35_500)
    assert table.loc["10kb", "cliffs_delta_vs_native"] > 0
    assert table.loc["10kb", "per_hop_reference_verdict"] == "PASS"
    assert table.loc["100kb", "per_hop_reference_verdict"] == "FAIL"
    assert table.loc["1kb", "wafer_pooled_loss"] == pytest.approx(1 / (30 * 60_000))
    assert "in-process path only" in table.loc["1kb", "claim_boundary"]
    assert "10 KiB packet limit" in table.loc["1kb", "claim_boundary"]


def test_payload_table_requires_full_n_unless_diagnostic() -> None:
    records = payload_records()
    with pytest.raises(ValueError, match="30 independent runs"):
        payload_table([record for record in records if record["condition"] != "native-10kb"])
    diagnostic = [
        record
        for record in payload_records(runs=2, sizes=("1kb",))
        if not (record["condition"] == "native-1kb" and record["run_index"] == 2)
    ]
    table = payload_table(diagnostic, canonical=False)
    assert len(table) == 1 and table["N_pairs"].iloc[0] == 1
    assert not table["thesis_evidence"].any()


def test_payload_loss_does_not_count_duplicates_as_delivered() -> None:
    records = payload_records()
    for record in records:
        if record["condition"] == "native-10kb" and record["run_index"] == 2:
            record.update({"received_unique": 59_995, "duplicates": 5})
    table = payload_table(records).set_index("condition")
    assert table.loc["10kb", "native_pooled_loss"] == pytest.approx(6 / (30 * 60_000))
    assert table.loc["10kb", "native_duplicates"] == 5
    assert table.loc["10kb", "wafer_duplicates"] == 0


def validation_records(p99_ns: list[int]) -> list[dict]:
    return [
        {"condition": "delay-50ms", "run_index": index, "p50_ns": 50_100_000, "p99_ns": p99}
        for index, p99 in enumerate(p99_ns, start=1)
    ]


def test_validation_gate_uses_the_runner_p99_band() -> None:
    table = validation_gate_table(validation_records([55_017_471] + [50_300_000] * 29))
    assert table.loc[0, "gate_passed"] and table.loc[0, "runs_inside_band"] == 30
    table = validation_gate_table(validation_records([55_017_472] + [50_300_000] * 29))
    assert not table.loc[0, "gate_passed"] and table.loc[0, "runs_inside_band"] == 29


def test_validation_gate_requires_full_n_unless_diagnostic() -> None:
    with pytest.raises(ValueError, match="30 independent runs"):
        validation_gate_table(validation_records([50_300_000] * 29))
    assert not validation_gate_table(validation_records([50_300_000] * 2), canonical=False)["thesis_evidence"].any()


def test_density_table_compares_components_with_the_measured_floor() -> None:
    rows = [
        {"plugin": "pass-through", "wasm_bytes": "65536", "wasm_kb": "64.0"},
        {"plugin": "tensor-prep", "wasm_bytes": "262144", "wasm_kb": "256.0"},
    ]
    floor = {"base": "scratch", "image_bytes": 524_288}
    table = density_table(rows, floor).set_index("plugin")
    assert table.loc["pass-through", "container_floor_bytes"] == 524_288
    assert table.loc["pass-through", "floor_to_wasm_ratio"] == pytest.approx(8)
    assert table.loc["tensor-prep", "floor_to_wasm_ratio"] == pytest.approx(2)
    assert "measured" in table.loc["tensor-prep", "estimator"]
    assert "estimate" not in table.loc["tensor-prep", "estimator"]
    assert table["thesis_evidence"].all()
    assert not density_table(rows, floor, canonical=False)["thesis_evidence"].any()
    with pytest.raises(ValueError, match="one positive size"):
        density_table(rows + rows[:1], floor)
    for unmeasured in ({**floor, "base": "alpine"}, {**floor, "image_bytes": "50"}, {"base": "scratch"}):
        with pytest.raises(ValueError, match="measured FROM scratch"):
            density_table(rows, unmeasured)


def startup_records(runs: int = 30, conditions: tuple[str, ...] | None = None) -> list[dict]:
    base = {"small": 40_000_000, "medium": 60_000_000, "large": 120_000_000}
    conditions = conditions or tuple(f"{tier}-{state}" for tier in base for state in ("cold", "warm"))
    records = []
    for condition in conditions:
        tier, state = condition.rsplit("-", 1)
        for run in range(1, runs + 1):
            total = base[tier] * (3 if state == "cold" else 1) + run * 1_000
            phases = dict.fromkeys(("process_config", "instantiation", "pipeline_setup", "first_process"), total // 10)
            phases["component_load_compile"] = total - 4 * (total // 10) - 1_000_000
            records.append(
                {
                    "condition": condition,
                    "run_index": run,
                    "total_wall_duration_ns": total,
                    "phases_ns": phases,
                    "compiled_component_cache": {"mode": "disabled", "hit": False},
                }
            )
    return records


def test_startup_table_contrasts_cold_with_warm_per_tier() -> None:
    table = startup_table(startup_records()).set_index("condition")
    assert len(table) == 6
    assert table.loc["large-cold", "median_cold_minus_warm_ns"] == pytest.approx(240_000_000)
    assert table.loc["large-cold", "N_pairs"] == 30
    assert table.loc["large-cold", "cliffs_delta_vs_warm"] == 1.0
    assert pd.isna(table.loc["small-warm", "median_cold_minus_warm_ns"])
    assert table["median_unmeasured_ns"].eq(1_000_000).all()
    assert table["compiled_cache_hits"].eq(0).all()
    assert "no compiled-cache claim" in table.loc["small-cold", "claim_boundary"]


def test_startup_table_requires_full_n_unless_diagnostic() -> None:
    with pytest.raises(ValueError, match="30 independent runs"):
        startup_table(startup_records(conditions=("small-cold", "small-warm")))
    table = startup_table(startup_records(runs=2, conditions=("small-cold",)), canonical=False)
    assert len(table) == 1 and pd.isna(table["median_cold_minus_warm_ns"].iloc[0])


def test_startup_cold_minus_warm_pairs_runs_by_index() -> None:
    records = startup_records()
    for record in records:
        if record["condition"] in {"small-cold", "small-warm"}:
            cold_penalty = 5_000_000 if record["condition"] == "small-cold" else 0
            record["total_wall_duration_ns"] = 40_000_000 + record["run_index"] * 1_000_000 + cold_penalty
    row = startup_table(records).set_index("condition").loc["small-cold"]
    assert row["median_cold_minus_warm_ns"] == pytest.approx(5_000_000)
    assert (row["difference_ci95_low_ns"], row["difference_ci95_high_ns"]) == pytest.approx((5_000_000, 5_000_000))
    assert row["hodges_lehmann_shift_ns"] == pytest.approx(5_000_000)


def test_bucket_band_summarises_each_bucket_across_runs() -> None:
    band = bucket_band([-100_000_000, 0, 100_000_000], [[100, 50, 90], [100, 70, 100], [100, 60, 95]])
    assert band["offset_s"].tolist() == [-0.1, 0.0, 0.1]
    assert band["median"].tolist() == [100, 60, 95]
    assert band.loc[1, "p25"] == 55 and band.loc[1, "p75"] == 65
    assert band["N_runs"].eq(3).all()
    with pytest.raises(ValueError, match="one value per bucket"):
        bucket_band([0, 1], [[1, 2], [1]])


def test_runtime_exit_in_a_containment_run_counts_as_not_contained(tmp_path: Path) -> None:
    batch = tmp_path / "e-iso-6/rpi5-batch-a"
    for record in containment_records():
        if record["experiment"] != "e-iso-6":
            continue
        leaf = batch / "panic" / f"run-{record['run_index']:02d}-attempt-01"
        leaf.mkdir(parents=True)
        if record["run_index"] == 7:
            receipt = {"status": "failed", "failure_class": "sut_outcome", "reasons": ["runtime-exit"]}
        else:
            receipt = {"status": "passed"}
            (leaf / "containment.json").write_text(json.dumps(record))
        (leaf / "canonical-status.json").write_text(json.dumps(receipt))
    records = [record for record in containment_records() if record["experiment"] != "e-iso-6"]
    records += [
        {**value, "experiment": "e-iso-6"} for value in admitted_runs(batch, "containment.json")
    ]

    table = containment_table(records).set_index("experiment")

    assert table.loc["e-iso-6", "N_runs"] == 30
    assert table.loc["e-iso-6", "contained_runs"] == 29
    assert table.loc["e-iso-6", "runs_stopped_early"] == 1
    assert not table.loc["e-iso-6", "all_contained"]
    assert table.drop("e-iso-6")["all_contained"].all()


def test_one_lost_message_fails_the_burst_swap_zero_loss_criterion() -> None:
    runs = swap4_runs()
    runs[4].update(loss=1, sut_outcome_reasons=["message-loss"])

    row = swap4_table(runs).iloc[0]

    assert row["N_runs"] == 30
    assert row["total_loss"] == 1
    assert row["lossless_runs"] == 29
    assert not row["zero_loss_and_duplication"]


def test_burst_swap_run_stopped_by_a_failed_swap_counts_against_zero_loss() -> None:
    runs = swap4_runs()
    runs[0] = {"condition": "burst-2x", "run_index": 1, "sut_outcome_reasons": ["swap-failed"]}

    row = swap4_table(runs).iloc[0]

    assert row["N_runs"] == 30
    assert row["N_events"] == 29
    assert row["runs_stopped_early"] == 1
    assert row["lossless_runs"] == 29
    assert not row["zero_loss_and_duplication"]


def test_hot_swap_disruption_run_the_runtime_did_not_survive_fails_zero_loss() -> None:
    runs = swap3_runs()
    runs[0] = {"condition": "wafer-hotswap", "run_index": 1, "sut_outcome_reasons": ["runtime-exit"]}

    table = swap3_table(runs).set_index("strategy")

    assert table.loc["wafer-hotswap", "N_runs"] == 30
    assert table.loc["wafer-hotswap", "runs_stopped_early"] == 1
    assert not table.loc["wafer-hotswap", "zero_loss_and_duplication"]
    assert table.loc["wafer-restart", "zero_loss_and_duplication"]


def test_failed_rollback_is_reported_as_an_unsuccessful_run() -> None:
    records = swap5_records()
    requests = records[2]["requests"]
    requests[3] = {**requests[3], "http_status": 500, "body": {"error": "trap"}}
    records[2] = {
        "condition": "process-trap-rollback",
        "run_index": 3,
        "requests": requests,
        "sut_outcome_reasons": ["rollback-failed"],
    }

    runs = failed_replacement_table(records)
    row = runs.set_index("run_index").loc[3]
    summary = failed_replacement_summary(runs).iloc[0]

    assert row["N_nested_events"] == 50
    assert row["rolled_back_events"] == 49
    assert not row["all_rolled_back"]
    assert row["sut_outcome_reasons"] == "rollback-failed"
    assert (summary["N_runs"], summary["runs_stopped_early"]) == (10, 1)
    assert not summary["all_rolled_back"]
    assert summary["sut_outcome_reasons"] == "rollback-failed"
    assert summary["runs_with_post_rollback_output"] == 9
    assert summary["median_first_use_rollback_ns"] == np.median([4 + 100 * run for run in range(1, 11) if run != 3])


def test_slow_policy_loss_is_admitted_and_reported() -> None:
    records = backpressure_records()
    lossy = next(record for record in records if record["policy"] == "slow")
    lossy["counts"].update(accepted=999, processed=999, delivered=999)
    lossy["rates_msg_s"].update(accepted=999.0, processed=999.0)
    lossy["sequence"].update(received=999, gaps=1)
    lossy["accounting"]["reconciled"] = False
    lossy["sut_outcome_reasons"] = ["message-loss"]

    slow = backpressure_table(records).set_index("policy").loc["slow"]

    assert slow["N_runs"] == 30
    assert slow["sut_outcome_runs"] == 1
    assert slow["total_attempted"] - slow["total_delivered"] == 1


def test_capacity_tables_count_a_run_the_system_under_test_failed() -> None:
    summary = capacity_summary()
    top = summary["systems"]["wafer"]["rates"][3]
    top.update(run_count=29, sut_outcome_runs=1, classification="bad")
    for metric in top["run_summary"].values():
        metric["values"].pop()
    top["normalized_p99"]["values"].pop()

    rates, _ = capacity_tables(summary)

    row = rates[(rates["system"] == "wafer") & (rates["offered_rate_msg_s"] == 15_000)].iloc[0]
    assert (row["N_runs"], row["sut_outcome_runs"]) == (30, 1)
    assert not row["delivery_good"]


def test_capacity_tables_report_a_rate_where_every_run_failed() -> None:
    summary = capacity_summary()
    top = summary["systems"]["wafer"]["rates"][3]
    top.update(
        run_count=0,
        sut_outcome_runs=30,
        classification="bad",
        pooled_loss=None,
        mean_achieved_ratio=None,
        run_summary=None,
        normalized_p99=None,
    )

    rates, _ = capacity_tables(summary)

    row = rates[(rates["system"] == "wafer") & (rates["offered_rate_msg_s"] == 15_000)].iloc[0]
    assert (row["N_runs"], row["sut_outcome_runs"]) == (30, 30)
    assert not row["delivery_good"]
    assert pd.isna(row["median_p99_ns"]) and pd.isna(row["pooled_loss"])


def test_branch_isolation_counts_an_attack_run_the_runtime_did_not_survive() -> None:
    records = branch_records({"control": 1_000, "panic-attack": 1_000, "epoch-loop-attack": 1_000})
    crashed = next(
        index
        for index, record in enumerate(records)
        if record["condition"] == "panic-attack" and record["run_index"] == 7
    )
    records[crashed] = {
        "condition": "panic-attack",
        "run_index": 7,
        "sut_outcome_reasons": ["runtime-exit"],
    }

    table = branch_isolation_table(records).set_index("condition")

    assert table.loc["panic-attack", "N_runs"] == 30
    assert table.loc["panic-attack", "runs_stopped_early"] == 1
    assert table.loc["panic-attack", "drop_verdict"] == "PASS"
    assert table.loc["panic-attack", "verdict"] == "FAIL"
    assert table.loc["epoch-loop-attack", "verdict"] == "PASS"


def test_recovery_table_counts_a_run_the_runtime_did_not_survive() -> None:
    records = recovery_records()
    records[4] = {"condition": "panic-recovery", "run_index": 5, "sut_outcome_reasons": ["runtime-exit"]}

    row = recovery_table(records).iloc[0]

    assert (row["N_runs"], row["runs_stopped_early"], row["recovery_samples"]) == (30, 1, 87)


EKUIPER_MEDIAN_P95_NS = 122_015.5


def wafer_target_row(edit) -> pd.Series:
    records = percentile_runs(("wafer", "native", "ekuiper"))
    for record in records:
        if record["condition"] == "wafer":
            edit(record)
    table = target_latency_table(records).set_index("condition")
    assert table.loc[["native", "ekuiper"], "verdict"].isna().all()
    return table.loc["wafer"]


@pytest.mark.parametrize(("factor", "verdict"), [(1.0, "PASS"), (2.0, "INCONCLUSIVE"), (3.0, "FAIL")])
def test_target_latency_ratio_verdict_reads_one_sided_bounds_over_run_pairs(factor: float, verdict: str) -> None:
    wafer_p95 = [factor * EKUIPER_MEDIAN_P95_NS + (run - 15.5) * 4_000 for run in range(1, 31)]
    wafer = wafer_target_row(lambda record: record.update(p95_ns=wafer_p95[record["run_index"] - 1]))
    _, low, high = median_shift_ci(
        wafer_p95, [122_000 + run for run in range(1, 31)], relative=True, paired=True, ci=0.9
    )
    assert (wafer["p95_ratio_verdict"], wafer["verdict"]) == (verdict, verdict)
    assert wafer["p95_ratio_threshold"] == 2.0
    assert wafer["p95_ratio_ci_half_width"] == pytest.approx((high - low) / 2)
    assert wafer["p95_ratio_estimate"] == wafer["median_ratio_vs_reference"]
    nearer = min((1 + low, 1 + high), key=lambda bound: abs(bound - 2.0))
    assert wafer["p95_ratio_flips_at"] == pytest.approx(
        {"PASS": 1 + high, "INCONCLUSIVE": nearer, "FAIL": 1 + low}[verdict]
    )


@pytest.mark.parametrize(
    ("lost", "verdict"), [((0, 0), "PASS"), ((0, 1_200), "INCONCLUSIVE"), ((3_000, 3_000), "FAIL")]
)
def test_target_latency_loss_verdict_resamples_runs(lost: tuple[int, int], verdict: str) -> None:
    wafer = wafer_target_row(
        lambda record: record.update(received_unique=60_000 - lost[record["run_index"] % 2])
    )
    assert wafer["loss_verdict"] == verdict
    assert wafer["loss_estimate"] == wafer["pooled_loss"]
    assert wafer["delivery_verdict"] == verdict
    assert wafer["verdict"] == verdict


@pytest.mark.parametrize(
    ("ratios", "verdict"),
    [((1.0, 1.0), "PASS"), ((1.0, 0.98), "INCONCLUSIVE"), ((0.95, 0.95), "FAIL")],
)
def test_target_latency_achieved_ratio_verdict_bounds_the_mean_over_runs(
    ratios: tuple[float, float], verdict: str
) -> None:
    wafer = wafer_target_row(
        lambda record: record.update(achieved_ratio=ratios[record["run_index"] % 2])
    )
    low, high = bootstrap_ci(np.asarray([ratios[run % 2] for run in range(1, 31)]), ci=0.9, statistic=np.mean)
    assert wafer["achieved_ratio_verdict"] == verdict
    assert wafer["achieved_ratio_estimate"] == wafer["mean_achieved_ratio"]
    assert wafer["achieved_ratio_ci_half_width"] == pytest.approx((high - low) / 2)
    assert wafer["delivery_verdict"] == verdict


def test_one_duplicate_fails_target_load_delivery_exactly() -> None:
    wafer = wafer_target_row(lambda record: record.update(duplicates=int(record["run_index"] == 1)))
    assert (wafer["loss_verdict"], wafer["achieved_ratio_verdict"]) == ("PASS", "PASS")
    assert (wafer["duplicates_verdict"], wafer["duplicates_estimate"], wafer["duplicates_threshold"]) == ("FAIL", 1, 0)
    assert (wafer["delivery_verdict"], wafer["verdict"]) == ("FAIL", "FAIL")


def test_branch_isolation_is_inconclusive_when_the_drop_bounds_straddle_the_threshold() -> None:
    records = branch_records({"control": 1_000, "panic-attack": 990, "epoch-loop-attack": 900})
    for record in records:
        rate = {"control": 1_000, "panic-attack": 990, "epoch-loop-attack": 900}[record["condition"]]
        record["branches"]["branch_a"]["throughput"]["mean_messages_per_second"] = (
            rate + 2 * (record["run_index"] - 15.5)
        )
    table = branch_isolation_table(records).set_index("condition")
    attack = table.loc["panic-attack"]
    assert attack["throughput_drop_percent"] == pytest.approx(1.0)
    assert (attack["drop_verdict"], attack["verdict"]) == ("INCONCLUSIVE", "INCONCLUSIVE")
    assert attack["drop_threshold"] == 1.0
    assert attack["drop_estimate"] == attack["throughput_drop_percent"]
    assert attack["drop_flips_at"] - attack["drop_ci_half_width"] < 1.0 < attack["drop_flips_at"]
    assert table.loc["epoch-loop-attack", "verdict"] == "FAIL"
    assert pd.isna(table.loc["control", "verdict"]) and pd.isna(table.loc["control", "drop_verdict"])


def test_branch_isolation_drop_is_pending_without_a_control_measurement() -> None:
    records = [
        {"condition": "control", "run_index": record["run_index"], "sut_outcome_reasons": ["runtime-exit"]}
        if record["condition"] == "control"
        else record
        for record in branch_records({"control": 1_000, "panic-attack": 995, "epoch-loop-attack": 995})
    ]
    records[-1] = {"condition": "epoch-loop-attack", "run_index": 30, "sut_outcome_reasons": ["runtime-exit"]}
    table = branch_isolation_table(records).set_index("condition")
    assert table.loc["panic-attack", "drop_verdict"] == "PENDING"
    assert table.loc["panic-attack", "verdict"] == "PENDING"
    assert table.loc["epoch-loop-attack", "drop_verdict"] == "PENDING"
    assert table.loc["epoch-loop-attack", "verdict"] == "FAIL"


SWAP3_DIPS = {
    "PASS": lambda run: 2.0,
    "INCONCLUSIVE": lambda run: 3.5 + 0.1 * run,
    "FAIL": lambda run: 10.0,
}


@pytest.mark.parametrize("verdict", list(SWAP3_DIPS))
def test_swap3_dip_verdict_reads_the_one_sided_upper_bound(verdict: str) -> None:
    runs = swap3_runs()
    for run in runs:
        if run["strategy"] == "wafer-hotswap":
            run["dip_percent"] = SWAP3_DIPS[verdict](run["run_index"])
    table = swap3_table(runs).set_index("strategy")
    wafer = table.loc["wafer-hotswap"]
    low, high = bootstrap_ci(
        np.asarray([SWAP3_DIPS[verdict](run) for run in range(1, 31)]), ci=0.9
    )
    assert (wafer["dip_verdict"], wafer["verdict"]) == (verdict, verdict)
    assert wafer["dip_threshold"] == 5.0
    assert wafer["dip_estimate"] == wafer["median_dip_percent"]
    assert wafer["dip_flips_at"] in (low, high)
    assert wafer["dip_ci_half_width"] == pytest.approx((high - low) / 2)
    comparators = ["wafer-restart", "ekuiper-rule-update", "ekuiper-make-before-break"]
    assert table.loc[comparators, "verdict"].isna().all()
    assert table.loc[comparators, "dip_verdict"].isna().all()


def test_swap3_loss_fails_the_hot_swap_whatever_the_dip() -> None:
    runs = swap3_runs()
    runs[0]["loss"] = 1
    wafer = swap3_table(runs).set_index("strategy").loc["wafer-hotswap"]
    assert (wafer["dip_verdict"], wafer["verdict"]) == ("PASS", "FAIL")


def test_swap3_dip_is_pending_when_no_hot_swap_run_kept_running() -> None:
    runs = [
        {"strategy": "wafer-hotswap", "run_index": run["run_index"], "sut_outcome_reasons": ["swap-failed"]}
        if run["strategy"] == "wafer-hotswap"
        else run
        for run in swap3_runs()
    ]
    wafer = swap3_table(runs).set_index("strategy").loc["wafer-hotswap"]
    assert (wafer["dip_verdict"], wafer["verdict"]) == ("PENDING", "FAIL")
    assert pd.isna(wafer["dip_flips_at"]) and pd.isna(wafer["dip_ci_half_width"])
    assert pd.isna(wafer["dip_estimate"])


@pytest.mark.parametrize(
    ("gaps", "verdict"),
    [
        ([run * 1_000_000 for run in range(1, 31)], "PASS"),
        ([150_000_000 if run > 28 else run * 1_000_000 for run in range(1, 31)], "INCONCLUSIVE"),
        ([200_000_000] * 30, "FAIL"),
    ],
)
def test_swap4_gap_verdict_bounds_the_across_run_p95(gaps: list[int], verdict: str) -> None:
    runs = swap4_runs()
    for run, gap in zip(runs, gaps, strict=True):
        run["sink_observed_output_gap_ns"] = gap
    row = swap4_table(runs).iloc[0]
    assert (row["p95_gap_verdict"], row["verdict"]) == (verdict, verdict)
    assert row["p95_gap_threshold"] == 100_000_000
    assert row["p95_sink_gap_ns"] == sorted(gaps)[28]
    assert row["p95_gap_estimate"] == row["p95_sink_gap_ns"]


def test_swap4_loss_fails_the_burst_swap_whatever_the_gap() -> None:
    runs = swap4_runs()
    runs[0]["sequence"]["duplicates"] = 1
    row = swap4_table(runs).iloc[0]
    assert (row["p95_gap_verdict"], row["verdict"]) == ("PASS", "FAIL")


def test_payload_reference_is_inconclusive_when_its_bounds_straddle_the_reference() -> None:
    records = payload_records()
    for record in records:
        if record["condition"] == "100kb":
            record["service_p50_ns"] -= 11_000
    row = payload_table(records).set_index("condition").loc["100kb"]
    assert row["boundary_p50_ns"] == pytest.approx(50_000)
    assert row["per_hop_reference_verdict"] == "INCONCLUSIVE"
    assert row["per_hop_reference_estimate"] == row["boundary_p50_ns"]
    assert row["per_hop_reference_threshold"] == 50_000
    assert row["per_hop_reference_ci_half_width"] == pytest.approx(1_000)
    assert "a reference, not a pass criterion" in row["threshold"]


def declare_threshold(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch, criterion: str, value: float | None
) -> None:
    """Point the analysis at a copy of the canonical matrix with one threshold changed or the table removed."""
    matrix = json.loads((paths._find_repo_root() / "eval/canonical-matrix.json").read_text())
    if value is None:
        del matrix["verdict_rules"]
    else:
        row = next(
            row for row in matrix["verdict_rules"]["thresholds"] if row["criterion"] == criterion
        )
        row["value"] = value
    (tmp_path / "eval").mkdir()
    (tmp_path / "eval/canonical-matrix.json").write_text(json.dumps(matrix))
    monkeypatch.setattr(paths, "_find_repo_root", lambda: tmp_path)


def test_a_changed_dip_threshold_in_the_matrix_changes_the_swap3_verdict(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    assert swap3_table(swap3_runs()).set_index("strategy").loc["wafer-hotswap", "verdict"] == "PASS"
    declare_threshold(tmp_path, monkeypatch, "e-swap-3-dip", 1.0)
    wafer = swap3_table(swap3_runs()).set_index("strategy").loc["wafer-hotswap"]
    assert (wafer["verdict"], wafer["dip_threshold"]) == ("FAIL", 1.0)
    assert "median dip < 1 percent" in wafer["threshold"]


def test_a_changed_competitive_ratio_in_the_matrix_moves_the_capacity_bracket(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    cells = (["good", "good", "good", "bad", "bad"], ["good", "good", "good", "good", "bad"])
    assert capacity_decision(*cells)["status"] == "CENSORED"
    declare_threshold(tmp_path, monkeypatch, "e-perf-10-competitive-ratio", 0.5)
    decision = capacity_decision(*cells)
    assert (decision["branch"], decision["status"], decision["threshold"]) == (
        "worst-case-pass",
        "PASS",
        0.5,
    )


def test_tables_refuse_a_matrix_without_declared_thresholds(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    declare_threshold(tmp_path, monkeypatch, "", None)
    with pytest.raises(ValueError, match="declares no verdict_rules"):
        swap3_table(swap3_runs())
