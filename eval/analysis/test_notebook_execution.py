import hashlib
import json
from pathlib import Path

import nbformat
from nbclient import NotebookClient

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
                    "classification": "good"
                    if rate < 16_000
                    else ("bad" if system == "mqtt-loopback" else "support-confounded"),
                    "run_summary": {
                        "achieved_rate_msg_s": {
                            "median": rate,
                            "bootstrap_median_ci95": [rate * 0.99, rate],
                        },
                        "p99_ns": {
                            "median": 100_000 + rate,
                            "bootstrap_median_ci95": [99_000, 101_000],
                        },
                    },
                    "normalized_p99": {
                        "median": 1.0 if rate < 8_000 else 2.1,
                        "bootstrap_median_ci95": [0.9, 2.2],
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
        "e-perf-4": ["120b", "1kb", "10kb", "100kb"],
        "e-perf-5": ["wafer", "native"],
        "e-perf-6": ["depth-1", "depth-3", "depth-5", "depth-10"],
        "e-perf-7": ["neither", "fuel-only", "epoch-only", "both"],
        "e-perf-8": ["depth-1", "depth-3", "depth-5", "depth-10"],
    }
    for experiment, experiment_values in experiment_conditions.items():
        for condition in experiment_values:
            p50 = conditions[condition]
            leaf = root / experiment / condition / "run-01"
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
            if experiment == "e-perf-6":
                (leaf / "memory.csv").write_text(
                    "elapsed_ms,rss_bytes\n0,67108864\n1000,67108864\n"
                )

    hotswap_events = [
        {
            "http_total_ns": 2_000_000,
            "sink_observed_output_gap_ns": 1_000_000,
        }
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
    write_passed_artifact(
        root / "e-swap-4" / "burst-2x" / "run-01",
        "burst-timeline.json",
        {
            "schema_version": 1,
            "successful_swaps": 1,
            "sink_observed_output_gap_ns": 1_000_000,
            "loss": 0,
            "sequence": {"duplicates": 0},
            "internal_swap_phases_ns": {
                "compile_ns": 1,
                "instantiate_ns": 2,
                "signal_ns": 3,
                "ack_ns": 4,
                "convergence_ns": 5,
            },
        },
    )
    for strategy in ("wafer-hotswap", "wafer-restart", "ekuiper-restart"):
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
    for run in (1, 2):
        write_passed_artifact(
            root / "e-iso-4" / "infinite-loop" / f"run-{run:02d}",
            "containment.json",
            {
                "condition": "infinite-loop",
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
                    "branches": {
                        "branch_a": {
                            "throughput": {"mean_messages_per_second": 1_000},
                            "latency_ns": {"p95": 120_000},
                        }
                    },
                },
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
    for run in (1, 2):
        write_passed_artifact(
            root / "e-backpressure" / "saturated" / f"run-{run:02d}",
            "backpressure.json",
            {
                "classification": "saturated-and-drained",
                "peak_occupancy": 1,
                "rates_msg_s": {
                    "offered": 1_000,
                    "accepted": 150,
                    "processed": 150,
                    "drained": 140,
                },
                "memory": {"within_limit": True},
            },
        )
    for tier in ("small", "medium", "large"):
        for cache_state in ("cold", "warm"):
            for run in (1, 2):
                write_passed_artifact(
                    root / "e-perf-9" / f"{tier}-{cache_state}" / f"run-{run:02d}",
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


def execute_notebooks(monkeypatch, fixture: Path) -> list[str]:
    experiment_paths = {
        "E_VAL_1_DIR": "e-val-1",
        "E_PERF_1_DIR": "e-perf-1",
        "E_PERF_2_DIR": "e-perf-2",
        "E_PERF_3_DIR": "e-perf-3",
        "E_PERF_4_DIR": "e-perf-4",
        "E_PERF_5_RPI_DIR": "e-perf-5",
        "E_PERF_5_X86_DIR": "e-perf-5",
        "E_PERF_6_DIR": "e-perf-6",
        "E_PERF_7_DIR": "e-perf-7",
        "E_PERF_8_DIR": "e-perf-8",
        "E_SWAP_1_DIR": "e-swap-1",
        "E_SWAP_2_DIR": "e-swap-2",
        "E_SWAP_3_DIR": "e-swap-3",
        "E_SWAP_4_DIR": "e-swap-4",
        "E_SWAP_6_DIR": "e-swap-6",
        "E_ISO_4_DIR": "e-iso-4",
        "E_ISO_7_DIR": "e-iso-7",
        "E_PERF_10_DIR": "e-perf-10",
        "E_BACKPRESSURE_DIR": "e-backpressure",
        "E_PERF_9_DIR": "e-perf-9",
    }
    for name, experiment in experiment_paths.items():
        (fixture / experiment).mkdir(parents=True, exist_ok=True)
        monkeypatch.setenv(name, str(fixture / experiment))
    monkeypatch.setenv("WAFER_SUMMARY_DIR", str(fixture))
    monkeypatch.setenv("WAFER_ANALYSIS_OUTPUT_DIR", str(fixture / "rendered"))
    monkeypatch.delenv("WAFER_EVAL_BATCH_ID", raising=False)

    cwd = Path(__file__).parent
    outputs = []
    for path in NOTEBOOKS:
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


def test_all_notebooks_execute_against_complete_focused_fixture(
    tmp_path, monkeypatch
) -> None:
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
        "06-fault-injection.ipynb": ("N=8", "N_runs"),
        "09-backpressure.ipynb": ("N=2", "N_runs"),
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
    assert (rendered / "e-perf-1-target-load.csv").is_file()
    assert (rendered / "e-perf-10-rate-estimates.csv").is_file()
    assert (rendered / "e-swap-4-burst.tex").is_file()

    for name in (
        "00-warmup-validation.ipynb",
        "02-per-hop-overhead.ipynb",
        "03-memory-scaling.ipynb",
        "04b-depth-scaling.ipynb",
        "05-hotswap-timeline.ipynb",
        "08-depth-scaling.ipynb",
        "09-saturation.ipynb",
    ):
        assert "[image/png]" in output_by_name[name], name


def test_all_notebooks_render_missing_conditions_as_pending(
    tmp_path, monkeypatch
) -> None:
    outputs = execute_notebooks(monkeypatch, tmp_path)
    assert len(outputs) == len(NOTEBOOKS)
    assert all("PENDING" in output for output in outputs)
