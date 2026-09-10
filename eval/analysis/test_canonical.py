from __future__ import annotations

import hashlib
import json
from pathlib import Path

import numpy as np
import pytest

from wafer_analysis.canonical import (
    FINAL_VISUAL_MANIFEST,
    candidate_capacity_table,
    candidate_depth_table,
    candidate_payload_table,
    candidate_swap_tables,
    capacity_tables,
    ekuiper_profile_tables,
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
                        "gc_runtime_metrics": {
                            "status": "unavailable",
                            "reason": "ekuiper-2.1.0-has-no-validated-gc-event-interface",
                        },
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


def test_canonical_ekuiper_profile_remains_single_release_only() -> None:
    summary = ekuiper_profile_summary()
    summary["records"][1]["source_git_sha"] = "b" * 40

    with pytest.raises(ValueError, match="mixes source revisions"):
        ekuiper_profile_tables(summary)


def test_ekuiper_profile_tables_accept_bounded_terminal_partial_intervals() -> None:
    for row_count in (61, 62):
        summary = ekuiper_profile_summary()
        summary["records"][0]["interval_alignment"]["row_count"] = row_count

        runs, pairs = ekuiper_profile_tables(summary)

        assert len(runs) == 30
        assert len(pairs) == 15

    summary["records"][0]["interval_alignment"]["row_count"] = 63
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
    summary["records"][0]["interval_alignment"]["row_count"] = 59
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
            "ack_ns": 4,
            "convergence_ns": 5,
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
