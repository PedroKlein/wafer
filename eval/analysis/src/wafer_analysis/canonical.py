"""Run-level summaries for final canonical evaluation artifacts."""

from __future__ import annotations

import hashlib
import math
import statistics
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
                "ack_ns",
                "convergence_ns",
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
        gc_runtime = record.get("gc_runtime_metrics")
        if gc_runtime != {
            "status": "unavailable",
            "reason": "ekuiper-2.1.0-has-no-validated-gc-event-interface",
        }:
            raise ValueError("eKuiper profile record overstates GC/runtime evidence")
        interval = record.get("interval_alignment", {})
        if (
            interval.get("clock") != "unix-epoch"
            or not 60 <= int(interval.get("row_count", 0)) <= 62
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
