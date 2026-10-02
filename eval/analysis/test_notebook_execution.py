import hashlib
import json
import shutil
from pathlib import Path

import nbformat
import pandas as pd
from nbclient import NotebookClient

from wafer_analysis.rollback import SWAP5_PLUGIN, build_post_rollback_continuity, build_swap5_rollback

NOTEBOOKS = sorted((Path(__file__).parent / "notebooks").glob("*.ipynb"))


def write_passed_artifact(directory: Path, name: str, value: object) -> None:
    directory.mkdir(parents=True, exist_ok=True)
    (directory / "canonical-status.json").write_text('{"status":"passed"}')
    (directory / name).write_text(json.dumps(value))


def capacity_envelope_fixture() -> dict:
    systems = {}
    for system in ("mqtt-loopback", "native", "wafer", "ekuiper"):
        systems[system] = {
            "complete": True,
            "delivery_ceiling": {"rate_msg_s": 15_000, "censoring": "right-censored"},
            "normalized_p99_knee": {"rate_msg_s": 8_000, "censoring": "none"},
            "support_censoring": {
                "from_rate_msg_s": 16_000,
                "highest_support_uncensored_rate_msg_s": 15_000,
            },
            "rates": [
                {
                    "rate_msg_s": rate,
                    "run_count": 30,
                    "pooled_loss": 0.0 if rate < 16_000 else 0.02,
                    "mean_achieved_ratio": 1.0 if rate < 16_000 else 0.97,
                    "total_duplicates": 0,
                    "classification": "good"
                    if rate < 16_000
                    else ("bad" if system == "mqtt-loopback" else "support-confounded"),
                    "run_summary": {
                        "achieved_rate_msg_s": {
                            "median": rate,
                            "values": [rate * (0.98 + index / 1_500) for index in range(30)],
                        },
                        "achieved_ratio": {
                            "values": [1.0 if rate < 16_000 else 0.97] * 30,
                        },
                        "loss": {"values": [0.0 if rate < 16_000 else 0.02] * 30},
                        "p99_ns": {
                            "median": 100_000 + rate,
                            "values": [90_000 + 1_000 * index for index in range(30)],
                        },
                    },
                    "normalized_p99": {
                        "median": 1.0 if rate < 8_000 else 2.1,
                        "values": [0.9 + 0.05 * index for index in range(30)],
                    },
                }
                for rate in (1_000, 4_000, 8_000, 15_000, 16_000)
            ],
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


def rate_buckets(start_ns: int, count: int, width_ns: int, rate) -> list[dict]:
    return [
        {"start_offset_ns": start_ns + index * width_ns, "end_offset_ns": start_ns + (index + 1) * width_ns, "rate_msg_s": rate(start_ns + index * width_ns)}
        for index in range(count)
    ]


def write_swap5_fixture(leaf: Path) -> None:
    started = 1_700_000_000_000_000_000
    requests = [
        {
            "event_index": index,
            "plugin": SWAP5_PLUGIN,
            "request_started_ns": started + index * 2_000_000_000,
            "request_finished_ns": started + index * 2_000_000_000 + 30_000_000,
            "http_status": 200,
            "body": {
                "status": "rolled_back",
                "compile_cache": "compiled" if index == 0 else "memory_hit",
                "timeline": {"compile_ns": 1_000, "instantiate_ns": 2_000, "signal_ns": 3_000, "rollback_ns": 4_000_000 + index},
            },
        }
        for index in range(50)
    ]
    intervals = {
        "rows": [
            {
                "interval_start_unix_epoch_ns": started + second * 1_000_000_000,
                "interval_end_unix_epoch_ns": started + (second + 1) * 1_000_000_000,
                "throughput_messages": 1_000,
                "throughput_messages_per_second": 1_000.0,
            }
            for second in range(-5, 110)
        ]
    }
    sequence = {"expected": 115_000, "received": 115_000, "gaps": 0, "duplicates": 0}
    write_passed_artifact(leaf, "swap_requests.json", requests)
    (leaf / "rollback.json").write_text(json.dumps(build_swap5_rollback(requests, sequence)))
    (leaf / "interval-metrics.json").write_text(json.dumps(intervals))
    (leaf / "post-rollback-continuity.json").write_text(
        json.dumps(build_post_rollback_continuity(requests, intervals, sequence, interval_metrics_sha256="a" * 64))
    )
    (leaf / "sequence.csv").write_text(
        "total_expected,total_received,received_unique,gap_msgs,duplicates_count\n115000,115000,115000,0,0\n"
    )


def build_complete_fixture(root: Path) -> None:
    conditions = {
        "delay-50ms": 50_000_000,
        "wafer": 120_000,
        "native": 100_000,
        "ekuiper": 140_000,
        "120b": 100_000,
        "1kb": 110_000,
        "10kb": 120_000,
        "100kb": 130_000,
        "native-120b": 90_000,
        "native-1kb": 95_000,
        "native-10kb": 100_000,
        "native-100kb": 105_000,
        "depth-1": 100_000,
        "depth-3": 120_000,
        "depth-5": 140_000,
        "depth-10": 180_000,
        "neither": 100_000,
        "fuel-only": 105_000,
        "epoch-only": 106_000,
        "both": 110_000,
    }
    experiment_conditions = {
        "e-val-1": ["delay-50ms"],
        "e-perf-1": ["wafer", "native", "ekuiper"],
        "e-perf-2": ["wafer", "native", "ekuiper"],
        "e-perf-3": ["depth-1", "depth-3", "depth-5", "depth-10"],
        "e-perf-4": [
            "120b",
            "1kb",
            "10kb",
            "100kb",
            "native-120b",
            "native-1kb",
            "native-10kb",
            "native-100kb",
        ],
        "e-perf-5": ["wafer", "native"],
        "e-perf-6": ["depth-1", "depth-3", "depth-5", "depth-10"],
        "e-perf-7": ["neither", "fuel-only", "epoch-only", "both"],
        "e-perf-8": ["depth-1", "depth-3", "depth-5", "depth-10"],
    }
    for experiment, experiment_values in experiment_conditions.items():
        for condition in experiment_values:
            p50 = conditions[condition]
            leaf = root / experiment / condition / "run-01-attempt-01"
            write_passed_artifact(
                leaf,
                "percentiles.json",
                {
                    "total_count": 60_000,
                    "p50_ns": p50,
                    "p95_ns": p50 + 20_000,
                    "p99_ns": p50 + 40_000,
                    "p999_ns": p50 + 80_000,
                },
            )
            (leaf / "sequence.csv").write_text(
                "total_expected,total_received,received_unique,gap_msgs,duplicates_count\n60000,60000,60000,0,0\n"
            )
            if experiment == "e-perf-1":
                (leaf / "sequence.csv").write_text("event_type,seq_start,seq_end,count\n")
                (leaf / "throughput.csv").write_text(
                    "timestamp_ns,messages_received,throughput_msg_s,duration_ns\n1,60000,1000.0,60000000000\n"
                )
                (leaf / "subscriber-metadata.json").write_text(
                    json.dumps(
                        {
                            "sequence_end_exclusive": 60_000,
                            "total_recorded": 60_000,
                            "sequence": {
                                "expected": 60_000,
                                "total_received": 60_000,
                                "received_unique": 60_000,
                                "total_gaps": 0,
                                "total_duplicates": 0,
                            },
                        }
                    )
                )
            if experiment == "e-perf-4":
                (leaf / "service-percentiles.json").write_text(
                    json.dumps(
                        {
                            "total_count": 60_000,
                            "p50_ns": p50 - 20_000,
                            "p95_ns": p50,
                            "p99_ns": p50 + 20_000,
                            "p999_ns": p50 + 60_000,
                        }
                    )
                )
            if experiment == "e-perf-6":
                (leaf / "memory.csv").write_text(
                    "elapsed_ms,rss_bytes\n0,60000000\n30000,67108864\n31000,67108864\n"
                )

    density = root / "e-density-1" / "release-components" / "run-01-attempt-01"
    density.mkdir(parents=True)
    (density / "canonical-status.json").write_text('{"status":"passed"}')
    (density / "binary-sizes.csv").write_text(
        "plugin,wasm_bytes,wasm_kb\n"
        "pass-through,90000,87.9\n"
        "json-parse,180000,175.8\n"
    )
    (density / "container-floor.json").write_text(
        json.dumps({"base": "scratch", "platform": "linux/arm64", "image_bytes": 450_000})
    )

    for system in ("wafer", "native"):
        for rate in (1_000, 4_000):
            leaf = root / "e-perf-10" / system / f"rate-{rate:05d}" / "run-01-attempt-01"
            write_passed_artifact(leaf, "host-sidecar.json", {"sut_cpus": "1-3"})
            busy = rate // 100
            (leaf / "cpu-cores.csv").write_text(
                "timestamp_ns,cpu,user,nice,system,idle,iowait,irq,softirq,steal,frequency_hz\n"
                + "".join(
                    f"{stamp},{cpu},{stamp * (busy if cpu else 2 * busy)},0,0,{stamp * 100},0,0,0,0,2400000000\n"
                    for stamp in (1, 2)
                    for cpu in range(4)
                )
            )
    (root / "progress.jsonl").write_text(
        "".join(
            json.dumps(entry) + "\n"
            for index, experiment in enumerate(("e-val-1", "e-perf-1", "e-perf-1"))
            for entry in (
                {"timestamp": f"2026-10-01T0{index}:00:00Z", "event": "item-started", "item": f"{experiment}/c/run-0{index}", "failures": 0},
                {"timestamp": f"2026-10-01T0{index}:30:00Z", "event": "item-finished", "item": f"{experiment}/c/run-0{index}", "failures": 0, "temperature_c": 50.0 + index},
            )
        )
    )

    hotswap_events = [
        {
            "event_index": index,
            "compile_cache": "compiled" if index == 0 else "memory_hit",
            "compile_ns": 40_000_000 if index == 0 else 10_000,
            "instantiate_ns": 200_000,
            "signal_ns": 5_000,
            "replacement_adopted_ns": 100_000,
            "first_post_replacement_local_outcome_ns": 50_000,
            "http_total_ns": 2_000_000,
            "sink_observed_output_gap_ns": 1_000_000,
        }
        for index in range(3)
    ]
    for experiment, condition, source in (
        ("e-swap-1", "steady", "synthetic/steady/run-01"),
        ("e-swap-2", "steady", "synthetic/steady/run-01"),
        ("e-swap-4", "burst-2x", "synthetic/burst/run-01"),
        ("e-swap-6", "steady", "synthetic/steady/run-01"),
    ):
        write_passed_artifact(
            root / experiment / condition / "run-01",
            "hotswap-analysis.json",
            {
                "measurement_source_leaf": source,
                "events": hotswap_events,
            },
        )
    (root / "e-swap-1" / "steady" / "run-01" / "sequence.csv").write_text(
        "total_expected,total_received,received_unique,gap_msgs,duplicates_count\n120000,120000,120000,0,0\n"
    )
    write_swap5_fixture(root / "e-swap-5" / "process-trap-rollback" / "run-01-attempt-01")
    write_passed_artifact(
        root / "e-swap-4" / "burst-2x" / "run-01",
        "burst-timeline.json",
        {
            "schema_version": 1,
            "successful_swaps": 1,
            "sink_observed_output_gap_ns": 1_000_000,
            "loss": 0,
            "sequence": {"duplicates": 0},
            "primary_received_events": 129_999,
            "drain_received_events": 1,
            "drain_first_offset_ns": 120_000_500_000,
            "drain_last_offset_ns": 120_000_500_000,
            "drain_duration_after_window_ns": 500_000,
            "max_arrival_offset_ns": 120_000_500_000,
            "drain_right_censored": False,
            "internal_swap_phases_ns": {
                "compile_ns": 1,
                "instantiate_ns": 2,
                "signal_ns": 3,
                "replacement_adopted_ns": 4,
                "first_post_replacement_local_outcome_ns": 5,
            },
        },
    )
    swap4_leaf = root / "e-swap-4" / "burst-2x" / "run-01"
    (swap4_leaf / "throughput-buckets.json").write_text(
        json.dumps(
            {
                "primary_buckets": rate_buckets(0, 1_200, 100_000_000, lambda start: 2_000.0 if 55e9 <= start < 65e9 else 1_000.0),
                "drain_buckets": rate_buckets(120_000_000_000, 100, 100_000_000, lambda start: 10.0 if start < 120.5e9 else 0.0),
            }
        )
    )
    for strategy in ("wafer-hotswap", "wafer-restart", "ekuiper-restart"):
        leaf = root / "e-swap-3" / strategy / "run-01"
        leaf.mkdir(parents=True, exist_ok=True)
        (leaf / "throughput-buckets.json").write_text(
            json.dumps({"buckets": rate_buckets(-10_000_000_000, 200, 100_000_000, lambda start: 980.0 if 0 <= start < 1e8 else 1_000.0)})
        )
        (leaf / "throughput-buckets-10ms.json").write_text(
            json.dumps({"buckets": rate_buckets(-2_000_000_000, 400, 10_000_000, lambda start: 500.0 if 0 <= start < 3e7 else 1_000.0)})
        )
        write_passed_artifact(
            root / "e-swap-3" / strategy / "run-01",
            "disruption-analysis.json",
            {
                "schema_version": 1,
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
            },
        )
    attacks = {
        "e-iso-1": "buffer-overflow",
        "e-iso-2": "cross-read",
        "e-iso-3": "fs-access",
        "e-iso-4": "infinite-loop",
        "e-iso-5": "memory-exhaust",
        "e-iso-6": "panic",
    }
    for experiment, condition in attacks.items():
        for run in (1, 2):
            write_passed_artifact(
                root / experiment / condition / f"run-{run:02d}-attempt-01",
                "containment.json",
                {
                    "experiment": experiment,
                    "condition": condition,
                    "contained": True,
                    "expected_mechanism": "traps_total",
                    "expected_count": 2,
                    "unexpected_outcomes": 0,
                    "runtime_panic": False,
                    "healthy_messages_out": 60_000,
                    "traps_total": 2,
                    "nodes": [{"recovery_count": 2}],
                },
            )
    for condition in ("control", "panic-attack", "epoch-loop-attack"):
        for run in (1, 2):
            write_passed_artifact(
                root / "e-iso-7" / condition / f"run-{run:02d}",
                "branch-isolation.json",
                {
                    "condition": condition,
                    "run_index": run,
                    "branches": {
                        "branch_a": {
                            "offered_messages": 60_000,
                            "lost_messages": 0,
                            "throughput": {"mean_messages_per_second": 1_000 - run},
                            "latency_ns": {"p95": 120_000 + run},
                        }
                    },
                },
            )
    for run in (1, 2):
        leaf = root / "e-iso-8" / "panic-recovery" / f"run-{run:02d}-attempt-02"
        write_passed_artifact(leaf, "recovery.json", {"sample_count": 3})
        (leaf / "recovery.csv").write_text(
            "node_id,sample_index,duration_ns\n"
            f"attack,0,{40_000 * run}\nattack,1,{50_000 * run}\nattack,2,{90_000 * run}\n"
        )
    for system in ("mqtt-loopback", "native", "wafer", "ekuiper"):
        for rate in (500, 1_000, 2_000, 4_000, 8_000, 16_000):
            for run in (1, 2):
                write_passed_artifact(
                    root / "e-perf-10" / f"{system}-{rate}" / f"run-{run:02d}",
                    "rate-sweep.json",
                    {
                        "system": system,
                        "offered_rate_msg_s": rate,
                        "achieved_rate_msg_s": rate,
                        "latency_ns": {"p95": 120_000, "p99": 140_000},
                        "loss_percent": 0,
                    },
                )
    write_passed_artifact(
        root / "e-perf-10" / "summary" / "run-01",
        "rate-sweep-summary.json",
        capacity_envelope_fixture(),
    )
    for policy in ("slow", "drop", "dead-letter"):
        for run in range(1, 31):
            delivered = 1_000 if policy == "slow" else 700
            dropped = 300 if policy == "drop" else 0
            dead_lettered = 300 if policy == "dead-letter" else 0
            equations = {
                "slow": "attempted = delivered",
                "drop": "attempted = delivered + dropped",
                "dead-letter": "attempted = delivered + dead_lettered + dlq_full + dlq_closed",
            }
            write_passed_artifact(
                root / "e-backpressure" / policy / f"run-{run:02d}",
                "backpressure.json",
                {
                    "schema_version": 2,
                    "experiment": "e-backpressure",
                    "condition": policy,
                    "run_index": run,
                    "sample_unit": "run",
                    "policy": policy,
                    "queue": "slow",
                    "classification": "saturated-and-drained",
                    "threshold_crossed": True,
                    "recovered": True,
                    "peak_occupancy": 1.0,
                    "occupancy_threshold": 0.8,
                    "recovery_threshold": 0.1,
                    "counts": {
                        "attempted": 1_000,
                        "accepted": delivered,
                        "processed": delivered,
                        "delivered": delivered,
                        "dropped": dropped,
                        "dead_lettered": dead_lettered,
                        "downstream_closed": 0,
                        "dlq_full": 0,
                        "dlq_closed": 0,
                        "outstanding": 0,
                    },
                    "rates_msg_s": {"offered": 1_000, "accepted": delivered, "processed": delivered, "drained": 140},
                    "sequence": {"offered": 1_000, "received": delivered, "gaps": 1_000 - delivered, "duplicates": 0},
                    "accounting": {"equation": equations[policy], "reconciled": True, "dlq_failures": {"full": 0, "closed": 0, "total": 0}},
                    "producer_progress": "backpressured" if policy == "slow" else "nonblocking",
                    "memory": {"within_limit": True},
                },
            )
    for tier in ("small", "medium", "large"):
        for cache_state in ("cold", "warm"):
            for run in (1, 2):
                write_passed_artifact(
                    root / "e-perf-9" / f"{tier}-{cache_state}" / f"run-{run:02d}-attempt-01",
                    "startup.json",
                    {
                        "cache_state": cache_state,
                        "total_wall_duration_ns": 1_000_000,
                        "compiled_component_cache": {"mode": "disabled", "hit": False},
                        "phases_ns": {
                            "process_config": 100_000,
                            "component_load_compile": 200_000,
                            "instantiation": 300_000,
                            "pipeline_setup": 200_000,
                            "first_process": 200_000,
                        },
                    },
                )


def notebook_output(notebook: dict) -> str:
    output = []
    for cell in notebook["cells"]:
        for item in cell.get("outputs", []):
            output.append(item.get("text", ""))
            data = item.get("data", {})
            output.append(data.get("text/plain", ""))
            if "image/png" in data:
                output.append("[image/png]")
    return "\n".join(output)


def execute_notebooks(monkeypatch, fixture: Path, notebooks: list[Path] = NOTEBOOKS) -> list[str]:
    experiment_paths = {
        "E_VAL_1_DIR": "e-val-1",
        "E_PERF_1_DIR": "e-perf-1",
        "E_PERF_1_JETSON_DIR": "e-perf-1",
        "E_PERF_1_X86_DIR": "e-perf-1",
        "E_PERF_2_DIR": "e-perf-2",
        "E_PERF_3_DIR": "e-perf-3",
        "E_PERF_4_DIR": "e-perf-4",
        "E_PERF_5_RPI_DIR": "e-perf-5",
        "E_PERF_5_JETSON_DIR": "e-perf-5",
        "E_PERF_5_X86_DIR": "e-perf-5",
        "E_PERF_6_DIR": "e-perf-6",
        "E_PERF_7_DIR": "e-perf-7",
        "E_PERF_8_DIR": "e-perf-8",
        "E_SWAP_1_DIR": "e-swap-1",
        "E_SWAP_2_DIR": "e-swap-2",
        "E_SWAP_3_DIR": "e-swap-3",
        "E_SWAP_4_DIR": "e-swap-4",
        "E_SWAP_5_DIR": "e-swap-5",
        "E_SWAP_6_DIR": "e-swap-6",
        "E_ISO_1_DIR": "e-iso-1",
        "E_ISO_2_DIR": "e-iso-2",
        "E_ISO_3_DIR": "e-iso-3",
        "E_ISO_4_DIR": "e-iso-4",
        "E_ISO_5_DIR": "e-iso-5",
        "E_ISO_6_DIR": "e-iso-6",
        "E_ISO_7_DIR": "e-iso-7",
        "E_ISO_8_DIR": "e-iso-8",
        "E_PERF_10_DIR": "e-perf-10",
        "E_BACKPRESSURE_DIR": "e-backpressure",
        "E_PERF_9_DIR": "e-perf-9",
        "E_DENSITY_1_DIR": "e-density-1",
    }
    for name, experiment in experiment_paths.items():
        (fixture / experiment).mkdir(parents=True, exist_ok=True)
        monkeypatch.setenv(name, str(fixture / experiment))
    monkeypatch.setenv("WAFER_SUMMARY_DIR", str(fixture))
    monkeypatch.setenv("WAFER_PROGRESS_JSONL", str(fixture / "progress.jsonl"))
    monkeypatch.setenv("WAFER_ANALYSIS_OUTPUT_DIR", str(fixture / "rendered"))
    monkeypatch.delenv("WAFER_EVAL_BATCH_ID", raising=False)

    cwd = Path(__file__).parent
    outputs = []
    for path in notebooks:
        notebook = nbformat.read(path, as_version=4)
        executed = NotebookClient(
            notebook,
            timeout=120,
            kernel_name="python3",
            resources={"metadata": {"path": str(cwd)}},
        ).execute()
        outputs.append(notebook_output(executed))
    return outputs


def raw_hashes(root: Path) -> dict[str, str]:
    return {
        str(path.relative_to(root)): hashlib.sha256(path.read_bytes()).hexdigest()
        for path in sorted(root.rglob("*"))
        if path.is_file() and "rendered" not in path.parts
    }


def test_all_notebooks_execute_against_complete_fixture(
    tmp_path, monkeypatch
) -> None:
    # A run-like name above the run directories must not be read as a run index.
    tmp_path = tmp_path / "run-99"
    build_complete_fixture(tmp_path)
    before = raw_hashes(tmp_path)
    outputs = execute_notebooks(monkeypatch, tmp_path)
    assert raw_hashes(tmp_path) == before
    assert len(outputs) == len(NOTEBOOKS)
    for path, output in zip(NOTEBOOKS, outputs):
        assert "PENDING" not in output, path.name
        assert "diagnostic" in output.lower() or "thesis_evidence" in output, path.name
        assert "unit" in output.lower(), path.name
        assert "uncertainty" in output.lower(), path.name

    output_by_name = dict(zip((path.name for path in NOTEBOOKS), outputs))
    hotswap_output = output_by_name["05-hotswap-timeline.ipynb"]
    assert "N=4" in hotswap_output
    assert "median_dip_percent" in hotswap_output
    assert "e-swap-1" in hotswap_output
    assert "e-swap-2" not in hotswap_output
    assert "e-swap-4" in hotswap_output
    assert "e-swap-6" not in hotswap_output

    expected_independent_runs = {
        "06-fault-injection.ipynb": ("N=2", "N=6", "N_runs"),
        "09-backpressure.ipynb": ("N=90", "N_runs"),
        "09-saturation.ipynb": ("N=600", "N_runs"),
        "10-aot-startup.ipynb": ("N=12", "N_runs"),
    }
    for name, expected in expected_independent_runs.items():
        assert all(value in output_by_name[name] for value in expected), name

    rendered = tmp_path / "rendered"
    assert (rendered / "e-perf-2/target-load-latency.pdf").stat().st_size > 1_000
    assert (rendered / "e-perf-2/target-load-latency.png").stat().st_size > 1_000
    assert (rendered / "e-perf-7/metering-decomposition.pdf").stat().st_size > 1_000
    assert (rendered / "e-perf-10/gateway-capacity-metrics.png").stat().st_size > 1_000
    assert (rendered / "e-swap-3/disruption-metrics.pdf").stat().st_size > 1_000
    assert (rendered / "rq2/containment.pdf").stat().st_size > 1_000
    assert (rendered / "rq2-containment.csv").is_file()
    assert "N=12" in output_by_name["06-fault-injection.ipynb"]
    assert (rendered / "rq1/startup-phases.pdf").stat().st_size > 1_000
    assert (rendered / "rq1-startup.csv").is_file()
    assert (rendered / "rq1/payload-boundary.pdf").stat().st_size > 1_000
    assert (rendered / "rq1-payload-boundary.csv").is_file()
    assert (rendered / "rq1/depth-scaling.pdf").stat().st_size > 1_000
    assert (rendered / "rq1-depth-latency-slope.csv").is_file()
    assert (rendered / "rq1/depth-rss.pdf").stat().st_size > 1_000
    assert (rendered / "rq1-depth-rss-slope.csv").is_file()
    assert (rendered / "rq2/branch-isolation.pdf").stat().st_size > 1_000
    assert (rendered / "rq2-branch-isolation.csv").is_file()
    assert (rendered / "rq2/recovery.pdf").stat().st_size > 1_000
    assert (rendered / "rq2-recovery.csv").is_file()
    assert (rendered / "e-perf-1-target-load.csv").is_file()
    contrast = pd.read_csv(rendered / "e-perf-1-wafer-native-contrast.csv")
    assert contrast[["statistic", "N_pairs", "difference_ns"]].values.tolist() == [
        ["p95", 1, 20_000],
        ["p50", 1, 20_000],
    ]
    concordance = pd.read_csv(rendered / "e-perf-1-replication-concordance.csv")
    assert set(concordance.host) == {"jetson", "x86"}
    assert concordance.concordance.eq("same-verdict").all()
    assert concordance.canonical_verdict.eq("PASS").all()
    overhead = pd.read_csv(rendered / "e-perf-5-wafer-native-contrast.csv")
    assert overhead.host.tolist() == ["rpi5", "jetson", "x86"]
    assert overhead.columns[0] == "host" and "status" not in overhead
    assert overhead.median_ratio.eq(1.2).all()
    assert overhead.runs_stopped_early.eq(0).all()
    assert (rendered / "e-perf-10-rate-estimates.csv").is_file()
    assert (rendered / "e-swap-4-burst.tex").is_file()
    assert (rendered / "rq3/swap-phases.pdf").stat().st_size > 1_000
    assert (rendered / "rq3-swap-phases.csv").is_file()
    assert (rendered / "rq3/rollback.pdf").stat().st_size > 1_000
    assert (rendered / "rq3/disruption-timeline.pdf").stat().st_size > 1_000
    assert (rendered / "rq3/swap-fine-timeline.pdf").stat().st_size > 1_000
    assert (rendered / "rq3/burst-timeline.pdf").stat().st_size > 1_000
    assert (rendered / "rq3-rollback.csv").is_file()
    assert (rendered / "rq3-rollback-runs.csv").is_file()
    assert (rendered / "rq3-swap-sequence.csv").is_file()
    assert (rendered / "rq1/validation-gate.pdf").stat().st_size > 1_000
    assert (rendered / "rq1-validation-gate.csv").is_file()
    assert "\\label{tab:rq1-validation-gate}" in (rendered / "rq1-validation-gate.tex").read_text()
    assert (rendered / "rq1/density.pdf").stat().st_size > 1_000
    density = (rendered / "rq1-density.csv").read_text().splitlines()
    assert density[0].startswith("plugin,wasm_bytes,container_floor_bytes,floor_to_wasm_ratio,")
    assert all(",450000," in row for row in density[1:])
    assert (rendered / "rq1/core-utilisation.pdf").stat().st_size > 1_000
    assert (rendered / "e-perf-10-core-utilisation.csv").is_file()
    assert (rendered / "campaign/temperature.pdf").stat().st_size > 1_000
    assert (rendered / "campaign-attempts.csv").is_file()
    assert (rendered / "campaign-timing.csv").is_file()
    assert (rendered / "e-backpressure-queue-pressure.csv").is_file()

    for name in (
        "00-warmup-validation.ipynb",
        "02-per-hop-overhead.ipynb",
        "03-memory-scaling.ipynb",
        "05-hotswap-timeline.ipynb",
        "08-depth-scaling.ipynb",
        "09-saturation.ipynb",
        "10-summary-stats.ipynb",
    ):
        assert "[image/png]" in output_by_name[name], name


def test_all_notebooks_render_missing_conditions_as_pending(
    tmp_path, monkeypatch
) -> None:
    outputs = execute_notebooks(monkeypatch, tmp_path)
    assert len(outputs) == len(NOTEBOOKS)
    assert all("PENDING" in output for output in outputs)


def test_depth_notebooks_render_a_single_depth_without_a_slope(tmp_path, monkeypatch) -> None:
    build_complete_fixture(tmp_path)
    for experiment in ("e-perf-3", "e-perf-6", "e-perf-8"):
        for depth in ("depth-3", "depth-5", "depth-10"):
            shutil.rmtree(tmp_path / experiment / depth)
    notebooks = [path for path in NOTEBOOKS if path.name in {"03-memory-scaling.ipynb", "08-depth-scaling.ipynb"}]
    outputs = execute_notebooks(monkeypatch, tmp_path, notebooks)
    assert all("PENDING" in output for output in outputs)
    assert (tmp_path / "rendered/rq1/depth-scaling.pdf").is_file()
    assert (tmp_path / "rendered/rq1/depth-rss.pdf").is_file()
