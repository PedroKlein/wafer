import json

import pytest

from wafer_analysis.host import attempts_table, core_utilisation, cpu_list, item_progress


def cpu_row(timestamp: int, cpu: int, busy: int, idle: int, iowait: int = 0) -> dict:
    return {
        "timestamp_ns": str(timestamp),
        "cpu": str(cpu),
        "user": str(busy),
        "nice": "0",
        "system": "0",
        "idle": str(idle),
        "iowait": str(iowait),
        "irq": "0",
        "softirq": "0",
        "steal": "0",
    }


def test_core_utilisation_uses_jiffy_differences_per_core() -> None:
    rows = [
        cpu_row(2, 0, 175, 1_000, 10),
        cpu_row(1, 0, 100, 900),
        cpu_row(1, 1, 50, 50),
        cpu_row(2, 1, 60, 140),
    ]
    table = core_utilisation(rows).set_index("cpu")
    assert table.loc[0, "busy_percent"] == pytest.approx(75 / 185 * 100)
    assert table.loc[0, "iowait_percent"] == pytest.approx(10 / 185 * 100)
    assert table.loc[1, "busy_percent"] == pytest.approx(10)


def test_core_utilisation_rejects_a_single_or_decreasing_sample() -> None:
    with pytest.raises(ValueError, match="cpu 0 needs two increasing samples"):
        core_utilisation([cpu_row(1, 0, 100, 900)])
    with pytest.raises(ValueError, match="cpu 0 needs two increasing samples"):
        core_utilisation([cpu_row(1, 0, 100, 900), cpu_row(2, 0, 90, 1_000)])


def test_attempts_table_counts_classes_retries_and_missing_units(tmp_path) -> None:
    matrix = {
        "final_campaign": {
            "attempt_policy": {"infrastructure_retries": 1, "gate_experiments": ["e-val-1"]}
        },
        "experiments": {
            "e-perf-1": {"conditions": ["wafer", "native"], "repetitions": 3},
            "e-val-1": {"conditions": ["delay-50ms"], "repetitions": 1},
        },
    }

    def attempt(path: str, receipt: dict | None, system: str = "wafer") -> None:
        directory = tmp_path / path
        directory.mkdir(parents=True)
        (directory / "metadata.json").write_text(json.dumps({"system": system}))
        if receipt is not None:
            (directory / "canonical-status.json").write_text(json.dumps(receipt))

    infrastructure = {
        "status": "failed",
        "failure_class": "infrastructure",
        "reasons": ["harness-error"],
    }
    outcome = {"status": "failed", "failure_class": "sut_outcome", "reasons": ["runtime-exit"]}
    attempt("perf/wafer/run-01-attempt-01", infrastructure)
    attempt("perf/wafer/run-01-attempt-02", {"status": "passed"})
    attempt("perf/wafer/run-02-attempt-01", outcome)
    attempt("perf/wafer/run-03-attempt-01", None)
    attempt("perf/wafer/run-03-attempt-02", infrastructure)
    attempt("perf/native/run-01-attempt-01", {"status": "passed"}, "native")
    (tmp_path / "perf/native/summary").mkdir()
    attempt("val/delay-50ms/run-01-attempt-01", None)

    table = attempts_table({"e-perf-1": tmp_path / "perf", "e-val-1": tmp_path / "val"}, matrix)

    columns = ["units", "attempts", "passed", "sut_outcome", "infrastructure", "retries", "missing"]
    rows = table.set_index(["experiment", "system", "condition"])[columns]
    assert rows.to_dict("index") == {
        ("e-perf-1", "native", "native"): dict(zip(columns, [3, 1, 1, 0, 0, 0, 2])),
        ("e-perf-1", "wafer", "wafer"): dict(zip(columns, [3, 5, 1, 1, 3, 2, 1])),
        ("e-val-1", "wafer", "delay-50ms"): dict(zip(columns, [1, 1, 0, 0, 1, 0, 1])),
    }


def test_item_progress_pairs_start_and_finish_and_flags_failures() -> None:
    progress = [
        {"timestamp": "2026-10-01T00:00:00Z", "event": "batch-started", "failures": 0},
        {"timestamp": "2026-10-01T00:00:00Z", "event": "item-started", "item": "e-val-1/delay-50ms/run-01", "failures": 0},
        {"timestamp": "2026-10-01T00:02:30Z", "event": "item-finished", "item": "e-val-1/delay-50ms/run-01", "failures": 0, "temperature_c": 48.5},
        {"timestamp": "2026-10-01T00:02:30Z", "event": "item-started", "item": "e-perf-1/wafer/run-01", "failures": 0},
        {"timestamp": "2026-10-01T00:04:30Z", "event": "item-finished", "item": "e-perf-1/wafer/run-01", "failures": 1, "temperature_c": 51.0},
        {"timestamp": "2026-10-01T00:04:30Z", "event": "item-started", "item": "e-perf-1/wafer/run-02", "failures": 1},
    ]
    table = item_progress(progress)
    assert table["experiment"].tolist() == ["e-val-1", "e-perf-1"]
    assert table["minutes"].tolist() == [2.5, 2.0]
    assert table["failed"].tolist() == [False, True]
    assert table["temperature_c"].tolist() == [48.5, 51.0]


def test_cpu_list_expands_ranges() -> None:
    assert cpu_list("1-3") == {1, 2, 3}
    assert cpu_list("0,2-3") == {0, 2, 3}
