#!/usr/bin/env python3

import csv
import importlib.util
import json
import subprocess
import sys
from pathlib import Path

import pytest

ROOT = Path(__file__).resolve().parents[3]
SPEC = importlib.util.spec_from_file_location(
    "interval_metrics", ROOT / "eval/scripts/lib/interval_metrics.py"
)
assert SPEC and SPEC.loader
INTERVALS = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(INTERVALS)


def write_csv(path: Path, fieldnames: list[str], rows: list[dict]) -> None:
    with path.open("w", newline="") as stream:
        writer = csv.DictWriter(stream, fieldnames=fieldnames)
        writer.writeheader()
        writer.writerows(rows)


def write_fixture(output: Path) -> None:
    output.mkdir(exist_ok=True)
    (output / "subscriber-metadata.json").write_text('{"total_recorded":3}\n')
    (output / "interval-latency.json").write_text(json.dumps({
        "schema_version": 1,
        "interval_clock": "monotonic-elapsed",
        "alignment_clock": "unix-epoch",
        "alignment_clock_purpose": "cross-process-alignment-only",
        "measurement_start_unix_epoch_ns": 10_000_000_000,
        "declared_measurement_duration_ns": 2_000_000_000,
        "bucket_width_ns": 1_000_000_000,
        "maximum_rows": 4,
        "row_count": 2,
        "aggregate_latency_count": 3,
        "late_arrivals": 0,
        "rows": [
            {
                "interval_start_ns": 0,
                "interval_end_ns": 1_000_000_000,
                "interval_start_unix_epoch_ns": 10_000_000_000,
                "interval_end_unix_epoch_ns": 11_000_000_000,
                "latency_count": 2,
                "latency_p50_ns": 10_000,
                "latency_p95_ns": 20_000,
                "latency_p99_ns": 20_000,
                "received_events": 2,
                "throughput_messages": 2,
                "duplicates": 0,
            },
            {
                "interval_start_ns": 1_000_000_000,
                "interval_end_ns": 2_000_000_000,
                "interval_start_unix_epoch_ns": 11_000_000_000,
                "interval_end_unix_epoch_ns": 12_000_000_000,
                "latency_count": 1,
                "latency_p50_ns": 30_000,
                "latency_p95_ns": 30_000,
                "latency_p99_ns": 30_000,
                "received_events": 2,
                "throughput_messages": 1,
                "duplicates": 1,
            },
        ],
    }, indent=2) + "\n")
    write_csv(
        output / "resource-usage.csv",
        ["timestamp_ns", "cpu_time_ticks", "rss_bytes", "process_count"],
        [
            {"timestamp_ns": 10_000_000_000, "cpu_time_ticks": 100, "rss_bytes": 1000, "process_count": 1},
            {"timestamp_ns": 10_500_000_000, "cpu_time_ticks": 110, "rss_bytes": 1200, "process_count": 1},
            {"timestamp_ns": 11_000_000_000, "cpu_time_ticks": 120, "rss_bytes": 1500, "process_count": 1},
            {"timestamp_ns": 11_500_000_000, "cpu_time_ticks": 130, "rss_bytes": 1400, "process_count": 1},
        ],
    )
    write_csv(
        output / "pi-telemetry.csv",
        ["timestamp_ns", "temperature_millicelsius", "rail_proxy_watts"],
        [
            {"timestamp_ns": 10_100_000_000, "temperature_millicelsius": 50000, "rail_proxy_watts": 4.0},
            {"timestamp_ns": 10_700_000_000, "temperature_millicelsius": 51000, "rail_proxy_watts": 6.0},
        ],
    )
    write_csv(
        output / "queue-depth.csv",
        ["elapsed_ns", "depth"],
        [
            {"elapsed_ns": 100_000_000, "depth": 3},
            {"elapsed_ns": 700_000_000, "depth": 7},
        ],
    )
    (output / "queue-depth-clock.json").write_text(json.dumps({
        "schema_version": 1,
        "elapsed_clock": "monotonic",
        "alignment_clock": "unix-epoch",
        "alignment_clock_purpose": "cross-process-alignment-only",
        "start_unix_epoch_ns": 10_000_000_000,
    }) + "\n")


def test_compose_interval_metrics_reconciles_and_marks_missing_samples(tmp_path: Path) -> None:
    write_fixture(tmp_path)

    destination = INTERVALS.compose_interval_metrics(tmp_path, clock_ticks=100)
    assert destination == tmp_path / "interval-metrics.json"
    value = json.loads(destination.read_text())

    assert value["row_count"] == 2
    assert sum(row["latency_count"] for row in value["rows"]) == 3
    assert value["rows"][0]["cpu_percent"] == {"status": "available", "value": 20.0}
    assert value["rows"][0]["rss_bytes"] == {"status": "available", "value": 1200}
    assert value["rows"][0]["pmic_internal_rail_proxy_watts"] == {
        "status": "available", "value": 5.0,
    }
    assert value["rows"][0]["temperature_millicelsius"] == {
        "status": "available", "value": 51000,
    }
    assert value["rows"][0]["throughput_messages_per_second"] == 2.0
    assert value["rows"][0]["queue_depth"] == {"status": "available", "value": 7}
    assert value["rows"][1]["queue_depth"] == {
        "status": "unavailable", "reason": "no-queue-sample-in-interval",
    }
    assert value["rows"][1]["pmic_internal_rail_proxy_watts"] == {
        "status": "unavailable", "reason": "no-host-telemetry-sample-in-interval",
    }
    INTERVALS.validate_interval_metrics(destination, aggregate_count=3)
    with pytest.raises(FileExistsError):
        INTERVALS.compose_interval_metrics(tmp_path, clock_ticks=100)


def test_compose_uses_fragment_aggregate_for_in_process_run(tmp_path: Path) -> None:
    write_fixture(tmp_path)
    (tmp_path / "subscriber-metadata.json").unlink()

    destination = INTERVALS.compose_interval_metrics(tmp_path, clock_ticks=100)

    assert destination == tmp_path / "interval-metrics.json"
    INTERVALS.validate_interval_metrics(destination, aggregate_count=3)


def test_interval_validation_rejects_forbidden_fields_and_excess_rows(tmp_path: Path) -> None:
    write_fixture(tmp_path)
    destination = INTERVALS.compose_interval_metrics(tmp_path, clock_ticks=100)
    value = json.loads(destination.read_text())
    value["rows"][0]["sequence_id"] = 1
    destination.write_text(json.dumps(value))
    with pytest.raises(ValueError, match="forbidden field sequence_id"):
        INTERVALS.validate_interval_metrics(destination)

    value["rows"][0].pop("sequence_id")
    value["maximum_rows"] = 1
    destination.write_text(json.dumps(value))
    with pytest.raises(ValueError, match="maximum rows"):
        INTERVALS.validate_interval_metrics(destination)


def test_timed_run_without_interval_fragment_fails_closed(tmp_path: Path) -> None:
    (tmp_path / "latency.hdr").write_bytes(b"hdr")
    with pytest.raises(ValueError, match="lacks interval-latency"):
        INTERVALS.compose_interval_metrics(tmp_path, required=True)


def test_cli_reports_composition_error_without_traceback(tmp_path: Path) -> None:
    (tmp_path / "latency.hdr").write_bytes(b"hdr")

    completed = subprocess.run(
        [sys.executable, str(ROOT / "eval/scripts/lib/interval_metrics.py"), "--required", str(tmp_path)],
        capture_output=True,
        text=True,
        check=False,
    )

    assert completed.returncode == 1
    assert "timed latency/throughput run lacks interval-latency.json" in completed.stderr
    assert "Traceback" not in completed.stderr


def test_interval_composition_rejects_population_and_clock_drift(tmp_path: Path) -> None:
    write_fixture(tmp_path)
    fragment = json.loads((tmp_path / "interval-latency.json").read_text())
    fragment["rows"][1]["latency_count"] = 2
    (tmp_path / "interval-latency.json").write_text(json.dumps(fragment))
    with pytest.raises(ValueError, match="population"):
        INTERVALS.compose_interval_metrics(tmp_path)

    fragment["rows"][1]["latency_count"] = 1
    fragment["rows"][1]["interval_start_unix_epoch_ns"] += 1
    (tmp_path / "interval-latency.json").write_text(json.dumps(fragment))
    with pytest.raises(ValueError, match="Unix alignment"):
        INTERVALS.compose_interval_metrics(tmp_path)

    fragment["rows"][1]["interval_start_unix_epoch_ns"] -= 1
    fragment["late_arrivals"] = 1
    (tmp_path / "interval-latency.json").write_text(json.dumps(fragment))
    with pytest.raises(ValueError, match="late arrivals"):
        INTERVALS.compose_interval_metrics(tmp_path)


def test_interval_validation_rejects_late_arrivals(tmp_path: Path) -> None:
    write_fixture(tmp_path)
    destination = INTERVALS.compose_interval_metrics(tmp_path, clock_ticks=100)
    value = json.loads(destination.read_text())
    value["late_arrivals"] = 1
    destination.write_text(json.dumps(value))

    with pytest.raises(ValueError, match="late arrivals"):
        INTERVALS.validate_interval_metrics(destination)
