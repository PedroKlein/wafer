import json
from pathlib import Path

import nbformat
from nbclient import NotebookClient


NOTEBOOKS = sorted((Path(__file__).parent / "notebooks").glob("*.ipynb"))


def write_passed_artifact(directory: Path, name: str, value: object) -> None:
    directory.mkdir(parents=True, exist_ok=True)
    (directory / "canonical-status.json").write_text('{"status":"passed"}')
    (directory / name).write_text(json.dumps(value))


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
        "passthrough": 110_000,
    }
    experiment_conditions = {
        "e-val-1": ["delay-50ms"],
        "e-perf-2": ["wafer", "native", "ekuiper"],
        "e-perf-3": ["depth-1", "depth-3", "depth-5", "depth-10"],
        "e-perf-4": ["120b", "1kb", "10kb", "100kb"],
        "e-perf-5": ["wafer", "native"],
        "e-perf-6": ["depth-1", "depth-3", "depth-5", "depth-10"],
        "e-perf-7": ["neither", "fuel-only", "epoch-only", "passthrough"],
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

    write_passed_artifact(
        root / "e-swap-1" / "steady" / "run-01",
        "hotswap-analysis.json",
        {
            "measurement_source_leaf": "synthetic/run-01",
            "events": [
                {
                    "http_total_ns": 2_000_000,
                    "sink_observed_output_gap_ns": 1_000_000,
                }
            ],
        },
    )
    write_passed_artifact(
        root / "e-iso-4" / "infinite-loop" / "run-01",
        "containment.json",
        {"condition": "infinite-loop", "traps_total": 2, "nodes": [{"recovery_count": 2}]},
    )
    for condition in ("control", "panic-attack", "epoch-loop-attack"):
        write_passed_artifact(
            root / "e-iso-7" / condition / "run-01",
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
            write_passed_artifact(
                root / "e-perf-10" / f"{system}-{rate}" / "run-01",
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
        {"status": "diagnostic"},
    )
    write_passed_artifact(
        root / "e-backpressure" / "saturated" / "run-01",
        "backpressure.json",
        {
            "classification": "saturated-and-drained",
            "peak_occupancy": 1,
            "rates_msg_s": {"offered": 1_000, "accepted": 150, "processed": 150, "drained": 140},
            "memory": {"within_limit": True},
        },
    )
    for tier in ("small", "medium", "large"):
        for cache_state in ("cold", "warm"):
            write_passed_artifact(
                root / "e-perf-9" / f"{tier}-{cache_state}" / "run-01",
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
            output.append(item.get("data", {}).get("text/plain", ""))
    return "\n".join(output)


def execute_notebooks(monkeypatch, fixture: Path) -> list[str]:
    experiment_paths = {
        "E_VAL_1_DIR": "e-val-1",
        "E_PERF_2_DIR": "e-perf-2",
        "E_PERF_3_DIR": "e-perf-3",
        "E_PERF_4_DIR": "e-perf-4",
        "E_PERF_5_RPI_DIR": "e-perf-5",
        "E_PERF_5_X86_DIR": "e-perf-5",
        "E_PERF_6_DIR": "e-perf-6",
        "E_PERF_7_DIR": "e-perf-7",
        "E_PERF_8_DIR": "e-perf-8",
        "E_SWAP_DIR": "e-swap-1",
        "E_ISO_4_DIR": "e-iso-4",
        "E_ISO_7_DIR": "e-iso-7",
        "E_PERF_10_DIR": "e-perf-10",
        "E_BACKPRESSURE_DIR": "e-backpressure",
        "E_PERF_9_DIR": "e-perf-9",
    }
    for name, experiment in experiment_paths.items():
        monkeypatch.setenv(name, str(fixture / experiment))
    monkeypatch.setenv("WAFER_SUMMARY_DIR", str(fixture))
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


def test_all_notebooks_execute_against_complete_focused_fixture(tmp_path, monkeypatch) -> None:
    build_complete_fixture(tmp_path)
    outputs = execute_notebooks(monkeypatch, tmp_path)
    assert len(outputs) == len(NOTEBOOKS)
    for path, output in zip(NOTEBOOKS, outputs):
        assert "PENDING" not in output, path.name
        assert "diagnostic" in output.lower() or "thesis_evidence" in output, path.name
        assert "unit" in output.lower(), path.name
        assert "uncertainty" in output.lower(), path.name

    hotswap_output = outputs[next(i for i, path in enumerate(NOTEBOOKS) if path.name == "05-hotswap-timeline.ipynb")]
    assert "N=1" in hotswap_output
    assert "e-swap-1" in hotswap_output
    assert "e-swap-2" not in hotswap_output
    assert "e-swap-4" not in hotswap_output
    assert "e-swap-6" not in hotswap_output


def test_all_notebooks_render_missing_conditions_as_pending(tmp_path, monkeypatch) -> None:
    outputs = execute_notebooks(monkeypatch, tmp_path)
    assert len(outputs) == len(NOTEBOOKS)
    assert all("PENDING" in output for output in outputs)
