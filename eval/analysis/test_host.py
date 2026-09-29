import json

import pytest

from wafer_analysis.host import attempt_ledger, core_utilisation, cpu_list, item_progress


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


def test_attempt_ledger_counts_retries_and_failed_attempts(tmp_path) -> None:
    def leaf(path: str, status: str) -> None:
        directory = tmp_path / path
        directory.mkdir(parents=True)
        (directory / "canonical-status.json").write_text(json.dumps({"status": status}))

    leaf("wafer/rate-01000/run-01-attempt-01", "failed")
    leaf("wafer/rate-01000/run-01-attempt-02", "passed")
    leaf("wafer/rate-01000/run-02-attempt-01", "passed")
    leaf("native/rate-01000/run-01-attempt-01", "failed")
    leaf("native/rate-01000/summary", "passed")

    (row,) = attempt_ledger({"e-perf-10": tmp_path}).to_dict("records")
    assert row == {
        "experiment": "e-perf-10",
        "runs": 3,
        "attempts": 4,
        "passed_runs": 2,
        "failed_attempts": 2,
        "retried_runs": 1,
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
