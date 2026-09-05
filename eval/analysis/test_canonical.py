from __future__ import annotations

import hashlib
import json
from pathlib import Path

import numpy as np
import pytest

from wafer_analysis.canonical import (
    FINAL_VISUAL_MANIFEST,
    capacity_tables,
    metering_table,
    swap3_table,
    swap4_table,
    target_latency_table,
    validate_visual_manifest,
)
from wafer_analysis.stats import bootstrap_ci, cliffs_delta


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
    assert wafer["pooled_loss"] == 0
    assert wafer["mean_achieved_ratio"] == 1
    assert wafer["median_achieved_rate_msg_s"] == 1_000
    assert bool(wafer["delivery_good"])
    assert "pooled loss <= 0.01" in wafer["threshold"]
    assert wafer["claim_boundary"] == "matched 1,000 msg/s target load; not capacity"
    assert table["thesis_evidence"].eq(True).all()


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
    assert (
        fuel["claim_boundary"]
        == "run-level metering ablation; no per-message inference"
    )


def capacity_summary() -> dict:
    systems = {}
    for system in ("mqtt-loopback", "native", "wafer", "ekuiper"):
        rates = []
        for rate in (1_000, 4_000, 8_000, 15_000, 16_000):
            rates.append(
                {
                    "rate_msg_s": rate,
                    "run_count": 30,
                    "pooled_loss": 0.0 if rate < 16_000 else 0.02,
                    "mean_achieved_ratio": 1.0 if rate < 16_000 else 0.97,
                    "classification": "good"
                    if rate < 16_000
                    else ("bad" if system == "mqtt-loopback" else "support-confounded"),
                    "run_summary": {
                        "achieved_rate_msg_s": {
                            "median": rate if rate < 16_000 else rate * 0.97,
                            "iqr": [rate * 0.99, rate],
                            "min": rate * 0.98,
                            "max": rate,
                            "bootstrap_median_ci95": [rate * 0.99, rate],
                        },
                        "p99_ns": {
                            "median": 100_000 + rate,
                            "iqr": [100_000, 120_000],
                            "min": 90_000,
                            "max": 130_000,
                            "bootstrap_median_ci95": [99_000, 101_000],
                        },
                    },
                    "normalized_p99": {
                        "median": 1.0 if rate < 8_000 else 2.1,
                        "bootstrap_median_ci95": [0.9, 2.2],
                    },
                }
            )
        systems[system] = {
            "complete": True,
            "delivery_ceiling": {"rate_msg_s": 15_000, "censoring": "right-censored"},
            "normalized_p99_knee": {"rate_msg_s": 8_000, "censoring": "none"},
            "support_censoring": {
                "from_rate_msg_s": 16_000,
                "highest_support_uncensored_rate_msg_s": 15_000,
            },
            "rates": rates,
        }
    return {
        "schema_version": 1,
        "experiment": "e-perf-10",
        "thesis_evidence": True,
        "sample_unit": "run",
        "required_runs_per_rate": 30,
        "rate_points_msg_s": [1_000, 4_000, 8_000, 15_000, 16_000],
        "systems": systems,
    }


def test_capacity_tables_keep_metrics_and_support_limitation_separate() -> None:
    rates, boundaries = capacity_tables(capacity_summary())
    assert len(rates) == 20
    assert rates["N_runs"].eq(30).all()
    assert {
        "offered_rate_msg_s",
        "median_achieved_rate_msg_s",
        "pooled_loss",
        "median_p99_ns",
        "median_normalized_p99",
    } <= set(rates.columns)
    assert "achieved_rate_msg_s" not in boundaries.columns
    assert {
        "delivery_ceiling_msg_s",
        "normalized_p99_knee_msg_s",
        "mqtt_support_path_limitation",
    } <= set(boundaries.columns)
    assert boundaries["claim_boundary"].str.contains("support").all()
    assert set(rates["classification"]) == {"good", "bad", "support-confounded"}


def test_capacity_tables_reject_schema_drift_from_the_producer() -> None:
    summary = capacity_summary()
    del summary["systems"]["wafer"]["rates"][0]["run_summary"]
    with pytest.raises(TypeError, match="rate summary is malformed"):
        capacity_tables(summary)

    summary = capacity_summary()
    summary["rate_points_msg_s"][-1] = 32_000
    with pytest.raises(ValueError, match="wrong common rate grid"):
        capacity_tables(summary)


def swap3_runs() -> list[dict]:
    return [
        {
            "run_index": run,
            "strategy": strategy,
            "baseline_rate_msg_s": 1_000,
            "event_min_rate_msg_s": 980,
            "dip_percent": 2.0,
            "interruption_ns": 100_000_000,
            "recovery_ns": 200_000_000,
            "recovery_right_censored": False,
            "action_duration_ns": 10_000_000,
            "loss": 0,
            "duplicates": 0,
        }
        for strategy in ("wafer-hotswap", "wafer-restart", "ekuiper-restart")
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
        == "upper bootstrap CI for median dip < 5%; zero loss; zero duplication"
    )
    assert wafer["dip_ci95_high_percent"] < 5


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
                "ack_ns": 4,
                "convergence_ns": 5,
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
    assert table.loc[0, "median_convergence_ns"] == 5
    assert table.loc[0, "runs_with_drain_arrivals"] == 15
    assert table.loc[0, "max_drain_arrival_offset_ns"] == 120_000_000_029
    assert table.loc[0, "drain_right_censored_runs"] == 0
    assert (
        table.loc[0, "threshold"]
        == "across-run p95 sink gap < 100 ms; zero full-run loss; zero duplication; no receive at or after 130 s"
    )
    broken = swap4_runs()
    broken[0]["successful_swaps"] = 2
    with pytest.raises(ValueError, match="one successful swap"):
        swap4_table(broken)


def test_visual_manifest_never_combines_incompatible_metrics() -> None:
    validate_visual_manifest(FINAL_VISUAL_MANIFEST)
    capacity = [
        row for row in FINAL_VISUAL_MANIFEST if row["experiment"] == "e-perf-10"
    ]
    assert {row["metric_group"] for row in capacity} == {
        "offered-versus-achieved-rate",
        "loss",
        "p99-latency",
        "delivery-ceiling",
        "normalized-p99-knee",
        "mqtt-support-path-limitation",
    }
    swap = [
        row for row in FINAL_VISUAL_MANIFEST if row["experiment"].startswith("e-swap")
    ]
    assert {row["metric_group"] for row in swap} >= {
        "internal-swap-phases",
        "sink-observed-gap",
        "event-aligned-dip",
        "action-duration",
        "recovery",
        "sequence-integrity",
        "burst-one-event-per-run",
    }
    invalid = [
        *FINAL_VISUAL_MANIFEST,
        {"experiment": "e-perf-10", "name": "bad", "metric_group": "offered-rate+p99"},
    ]
    with pytest.raises(ValueError, match="combined or unknown metric"):
        validate_visual_manifest(invalid)


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
