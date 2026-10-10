import json

import pytest

from wafer_analysis import paths
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


SECOND = 1_000_000_000


def test_core_utilisation_is_the_window_time_a_core_was_not_idle() -> None:
    # A tickless kernel counts idle and iowait exactly but samples busy time on the
    # tick: over 10 s at 100 ticks/s cpu 0 shows 20 busy ticks and 750 idle or iowait.
    rows = [
        cpu_row(10 * SECOND, 0, 20, 700, 50),
        cpu_row(0, 0, 0, 0),
        cpu_row(20 * SECOND, 0, 20, 1_700, 50),
        cpu_row(0, 1, 0, 0),
        cpu_row(10 * SECOND, 1, 0, 1_000),
        cpu_row(20 * SECOND, 1, 1_000, 1_000),
    ]
    table = core_utilisation(rows, 100, 0, 10 * SECOND).set_index("cpu")
    assert table.loc[0, "busy_percent"] == pytest.approx(25)
    assert table.loc[0, "iowait_percent"] == pytest.approx(5)
    assert table.loc[1, "busy_percent"] == pytest.approx(0)


def test_core_utilisation_rejects_a_single_or_decreasing_sample() -> None:
    with pytest.raises(ValueError, match="cpu 0 needs two increasing samples"):
        core_utilisation([cpu_row(SECOND, 0, 100, 900)], 100, 0, 2 * SECOND)
    with pytest.raises(ValueError, match="cpu 0 needs two increasing samples"):
        core_utilisation(
            [cpu_row(SECOND, 0, 100, 900), cpu_row(2 * SECOND, 0, 90, 1_000)], 100, 0, 2 * SECOND
        )
    with pytest.raises(ValueError, match="cpu 0 needs two increasing samples"):
        core_utilisation(
            [cpu_row(SECOND, 0, 100, 900), cpu_row(3 * SECOND, 0, 100, 1_100)], 100, 0, 2 * SECOND
        )


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


def test_attempts_table_schedules_the_bracket_rates_of_a_capacity_batch(
    tmp_path, monkeypatch
) -> None:
    monkeypatch.setattr(paths, "_find_repo_root", lambda: tmp_path)
    matrix = {
        "final_campaign": {
            "attempt_policy": {"infrastructure_retries": 1, "gate_experiments": ["e-val-1"]},
            "capacity_grid": {
                "common_rate_points_msg_s": [1_000],
                "bracket_rates": {"competitive_threshold": "competitive", "max_extra_rates": 2},
            },
        },
        "verdict_rules": {"thresholds": [{"criterion": "competitive", "value": 0.7}]},
        "experiments": {
            "e-perf-10": {"systems": ["wafer"], "rate_points_msg_s": [1_000], "repetitions": 2},
        },
    }
    ledger = tmp_path / "eval/results/canonical-batches/rpi5-a"
    ledger.mkdir(parents=True)
    (ledger / "batch.json").write_text(json.dumps({"capacity_brackets": {"rates_msg_s": [1_500]}}))
    batch = tmp_path / "eval/results/e-perf-10/rpi5-a"
    for rate in ("01000", "01500"):
        leaf = batch / f"wafer/rate-{rate}/run-01-attempt-01"
        leaf.mkdir(parents=True)
        (leaf / "metadata.json").write_text(json.dumps({"system": "wafer"}))
        (leaf / "canonical-status.json").write_text(json.dumps({"status": "passed"}))

    table = attempts_table({"e-perf-10": batch}, matrix)

    rows = table.set_index("condition")[["units", "passed", "missing"]]
    assert rows.to_dict("index") == {
        "wafer/rate-01000": {"units": 2, "passed": 1, "missing": 1},
        "wafer/rate-01500": {"units": 2, "passed": 1, "missing": 1},
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
