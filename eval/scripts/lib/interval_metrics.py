#!/usr/bin/env python3

from __future__ import annotations

import argparse
import csv
import json
import math
import os
import sys
from pathlib import Path
from typing import Any

INTERVAL_WIDTH_NS = 1_000_000_000
FORBIDDEN_FIELDS = {"message_id", "sequence_id", "per_message_timestamp"}


def available(value: int | float) -> dict[str, int | float | str]:
    return {"status": "available", "value": value}


def unavailable(reason: str) -> dict[str, str]:
    return {"status": "unavailable", "reason": reason}


def _load_json(path: Path) -> dict[str, Any]:
    value = json.loads(path.read_text())
    if not isinstance(value, dict):
        raise ValueError(f"{path.name} must contain an object")
    return value


def _read_csv(path: Path) -> list[dict[str, str]]:
    if not path.is_file():
        return []
    with path.open(newline="") as stream:
        return list(csv.DictReader(stream))


def _clock_start(path: Path) -> int | None:
    if not path.is_file():
        return None
    value = _load_json(path)
    if (
        value.get("schema_version") != 1
        or value.get("elapsed_clock") != "monotonic"
        or value.get("alignment_clock") != "unix-epoch"
        or value.get("alignment_clock_purpose")
        != "cross-process-alignment-only"
    ):
        raise ValueError(f"{path.name} has invalid clock labels")
    return int(value["start_unix_epoch_ns"])


def _validate_fragment(
    fragment: dict[str, Any], aggregate_count: int
) -> list[dict[str, Any]]:
    required = {
        "schema_version",
        "interval_clock",
        "alignment_clock",
        "alignment_clock_purpose",
        "measurement_start_unix_epoch_ns",
        "declared_measurement_duration_ns",
        "bucket_width_ns",
        "maximum_rows",
        "row_count",
        "aggregate_latency_count",
        "late_arrivals",
        "rows",
    }
    if required - fragment.keys():
        raise ValueError("interval latency fragment is missing required fields")
    if (
        fragment["schema_version"] != 1
        or fragment["interval_clock"] != "monotonic-elapsed"
        or fragment["alignment_clock"] != "unix-epoch"
        or fragment["alignment_clock_purpose"] != "cross-process-alignment-only"
        or fragment["bucket_width_ns"] != INTERVAL_WIDTH_NS
    ):
        raise ValueError("interval latency fragment has invalid clock or schema labels")
    duration_ns = int(fragment["declared_measurement_duration_ns"])
    if duration_ns <= 0:
        raise ValueError("interval latency duration must be positive")
    expected_maximum = math.ceil(duration_ns / INTERVAL_WIDTH_NS) + 2
    maximum_rows = int(fragment["maximum_rows"])
    rows = fragment["rows"]
    if not isinstance(rows, list) or fragment["row_count"] != len(rows):
        raise ValueError("interval latency row count differs")
    if duration_ns > 0 and not rows:
        raise ValueError("interval latency fragment has no interval rows")
    if maximum_rows != expected_maximum or len(rows) > maximum_rows:
        raise ValueError("interval latency cardinality exceeds its declared bound")
    if int(fragment["late_arrivals"]) != 0:
        raise ValueError("interval latency fragment contains late arrivals")
    previous_end = 0
    total = 0
    for index, row in enumerate(rows):
        if not isinstance(row, dict):
            raise ValueError("interval latency row must be an object")
        if FORBIDDEN_FIELDS & row.keys():
            raise ValueError("interval latency row contains per-message fields")
        start = int(row["interval_start_ns"])
        end = int(row["interval_end_ns"])
        width = end - start
        if (
            start != previous_end
            or width <= 0
            or width > INTERVAL_WIDTH_NS
            or (index < len(rows) - 1 and width != INTERVAL_WIDTH_NS)
        ):
            raise ValueError("interval latency rows have gaps, overlap, or invalid widths")
        if int(row["interval_start_unix_epoch_ns"]) != int(
            fragment["measurement_start_unix_epoch_ns"]
        ) + start:
            raise ValueError("interval latency Unix alignment differs from monotonic bounds")
        if int(row["interval_end_unix_epoch_ns"]) != int(
            fragment["measurement_start_unix_epoch_ns"]
        ) + end:
            raise ValueError("interval latency Unix alignment differs from monotonic bounds")
        count = int(row["latency_count"])
        if count == 0 and any(
            row[field] is not None
            for field in ("latency_p50_ns", "latency_p95_ns", "latency_p99_ns")
        ):
            raise ValueError("empty latency interval has percentile values")
        events = int(row["received_events"])
        if events != int(row["throughput_messages"]) + int(row["duplicates"]):
            raise ValueError("interval event, unique, and duplicate counts do not reconcile")
        if count < 0 or count > events:
            raise ValueError("interval latency count exceeds received events")
        total += count
        previous_end = end
    if int(fragment["aggregate_latency_count"]) != aggregate_count:
        raise ValueError("interval latency aggregate declaration differs")
    if total != aggregate_count:
        raise ValueError(
            f"interval latency population {total} differs from aggregate {aggregate_count}"
        )
    return rows


def _samples_in_window(
    rows: list[dict[str, str]], field: str, start_ns: int, end_ns: int
) -> list[dict[str, str]]:
    return [row for row in rows if start_ns <= int(row[field]) < end_ns]


def _process_metrics(
    rows: list[dict[str, str]], start_ns: int, end_ns: int, ticks_per_second: int
) -> tuple[dict[str, Any], dict[str, Any]]:
    selected = _samples_in_window(rows, "timestamp_ns", start_ns, end_ns)
    live = [row for row in selected if int(row["process_count"]) > 0]
    rss_values = [int(row["rss_bytes"]) for row in live if int(row["rss_bytes"]) > 0]
    rss = (
        available(max(rss_values))
        if rss_values
        else unavailable("no-valid-sut-rss-sample-in-interval")
    )
    before = [
        row for row in rows
        if int(row["timestamp_ns"]) <= start_ns and int(row["process_count"]) > 0
    ]
    after = [
        row for row in rows
        if int(row["timestamp_ns"]) >= end_ns and int(row["process_count"]) > 0
    ]
    cpu_rows = [before[-1], after[0]] if before and after else live
    if len(cpu_rows) < 2:
        return unavailable("insufficient-process-cpu-samples"), rss
    elapsed = int(cpu_rows[-1]["timestamp_ns"]) - int(cpu_rows[0]["timestamp_ns"])
    ticks = int(cpu_rows[-1]["cpu_time_ticks"]) - int(cpu_rows[0]["cpu_time_ticks"])
    if elapsed <= 0 or ticks < 0:
        return unavailable("invalid-process-cpu-sample-order"), rss
    cpu = ticks / ticks_per_second / (elapsed / INTERVAL_WIDTH_NS) * 100
    return available(cpu), rss


def _memory_metric(
    rows: list[dict[str, str]], clock_start_ns: int, start_ns: int, end_ns: int
) -> dict[str, Any]:
    selected = [
        row
        for row in rows
        if start_ns <= clock_start_ns + int(row["elapsed_ms"]) * 1_000_000 < end_ns
    ]
    values = [int(row["rss_bytes"]) for row in selected if int(row["rss_bytes"]) > 0]
    if not values:
        return unavailable("no-valid-rss-sample-in-interval")
    return available(max(values))


def _host_metrics(
    rows: list[dict[str, str]], start_ns: int, end_ns: int
) -> tuple[dict[str, Any], dict[str, Any]]:
    selected = _samples_in_window(rows, "timestamp_ns", start_ns, end_ns)
    if not selected:
        reason = "no-host-telemetry-sample-in-interval"
        return unavailable(reason), unavailable(reason)
    valid_power = [
        float(row["rail_proxy_watts"])
        for row in selected
        if float(row["rail_proxy_watts"]) > 0
    ]
    valid_temp = [
        int(row["temperature_millicelsius"])
        for row in selected
        if int(row["temperature_millicelsius"]) > 0
    ]
    power = (
        available(sum(valid_power) / len(valid_power))
        if valid_power
        else unavailable("no-valid-pmic-sample-in-interval")
    )
    temperature = (
        available(max(valid_temp))
        if valid_temp
        else unavailable("no-valid-temperature-sample-in-interval")
    )
    return power, temperature


def _queue_metric(
    rows: list[dict[str, str]], clock_start_ns: int, start_ns: int, end_ns: int
) -> dict[str, Any]:
    selected = [
        row
        for row in rows
        if start_ns <= clock_start_ns + int(row["elapsed_ns"]) < end_ns
    ]
    if not selected:
        return unavailable("no-queue-sample-in-interval")
    return available(max(int(row["depth"]) for row in selected))


def compose_interval_metrics(
    output: Path, *, clock_ticks: int | None = None, required: bool = False
) -> Path | None:
    fragments = sorted(output.rglob("interval-latency.json"))
    if not fragments:
        if required and (
            (output / "latency.hdr").is_file() or (output / "throughput.csv").is_file()
        ):
            raise ValueError("timed latency/throughput run lacks interval-latency.json")
        return None
    ticks_per_second = clock_ticks or int(os.sysconf("SC_CLK_TCK"))
    resource_rows = _read_csv(output / "resource-usage.csv")
    memory_rows = _read_csv(output / "memory.csv")
    telemetry_rows = _read_csv(output / "pi-telemetry.csv")
    queue_rows = _read_csv(output / "queue-depth.csv")
    memory_clock = _clock_start(output / "memory-clock.json") if memory_rows else None
    queue_clock = _clock_start(output / "queue-depth-clock.json") if queue_rows else None
    artifacts: list[Path] = []
    for fragment_path in fragments:
        parent = fragment_path.parent
        fragment = _load_json(fragment_path)
        aggregate_path = parent / "subscriber-metadata.json"
        if aggregate_path.is_file():
            aggregate_count = int(_load_json(aggregate_path)["total_recorded"])
        else:
            aggregate_count = int(fragment["aggregate_latency_count"])
        latency_rows = _validate_fragment(fragment, aggregate_count)
        start_unix_ns = int(fragment["measurement_start_unix_epoch_ns"])
        composed_rows = []
        for latency in latency_rows:
            start_elapsed_ns = int(latency["interval_start_ns"])
            end_elapsed_ns = int(latency["interval_end_ns"])
            start_ns = start_unix_ns + start_elapsed_ns
            end_ns = start_unix_ns + end_elapsed_ns
            cpu, process_rss = (
                _process_metrics(resource_rows, start_ns, end_ns, ticks_per_second)
                if resource_rows
                else (
                    unavailable("process-resource-source-unavailable"),
                    unavailable("process-resource-source-unavailable"),
                )
            )
            rss = process_rss
            if rss["status"] == "unavailable" and memory_rows:
                rss = (
                    _memory_metric(memory_rows, memory_clock, start_ns, end_ns)
                    if memory_clock is not None
                    else unavailable("memory-clock-source-unavailable")
                )
            power, temperature = (
                _host_metrics(telemetry_rows, start_ns, end_ns)
                if telemetry_rows
                else (
                    unavailable("host-telemetry-source-unavailable"),
                    unavailable("host-telemetry-source-unavailable"),
                )
            )
            queue = (
                _queue_metric(queue_rows, queue_clock, start_ns, end_ns)
                if queue_rows and queue_clock is not None
                else unavailable(
                    "queue-clock-source-unavailable" if queue_rows else "queue-source-unavailable"
                )
            )
            count = int(latency["latency_count"])
            composed_rows.append({
                "interval_start_ns": start_elapsed_ns,
                "interval_end_ns": end_elapsed_ns,
                "interval_start_unix_epoch_ns": start_ns,
                "interval_end_unix_epoch_ns": end_ns,
                "latency_count": count,
                "latency_p50_ns": (
                    available(int(latency["latency_p50_ns"]))
                    if count
                    else unavailable("empty-latency-interval")
                ),
                "latency_p95_ns": (
                    available(int(latency["latency_p95_ns"]))
                    if count
                    else unavailable("empty-latency-interval")
                ),
                "latency_p99_ns": (
                    available(int(latency["latency_p99_ns"]))
                    if count
                    else unavailable("empty-latency-interval")
                ),
                "received_events": int(latency["received_events"]),
                "throughput_messages": int(latency["throughput_messages"]),
                "throughput_messages_per_second": (
                    int(latency["throughput_messages"])
                    * INTERVAL_WIDTH_NS
                    / (end_elapsed_ns - start_elapsed_ns)
                ),
                "duplicates": int(latency["duplicates"]),
                "cpu_percent": cpu,
                "rss_bytes": rss,
                "pmic_internal_rail_proxy_watts": power,
                "temperature_millicelsius": temperature,
                "queue_depth": queue,
            })
        artifact = {
            key: fragment[key]
            for key in (
                "schema_version",
                "interval_clock",
                "alignment_clock",
                "alignment_clock_purpose",
                "measurement_start_unix_epoch_ns",
                "declared_measurement_duration_ns",
                "bucket_width_ns",
                "maximum_rows",
                "row_count",
                "aggregate_latency_count",
                "late_arrivals",
            )
        }
        artifact["sample_unit"] = "interval-within-run"
        sources = {"latency_throughput": fragment_path.name}
        if resource_rows:
            sources["process"] = "resource-usage.csv"
        if memory_rows and memory_clock is not None:
            sources["rss_fallback"] = "memory.csv"
        if telemetry_rows:
            sources["host"] = "pi-telemetry.csv"
        if queue_rows and queue_clock is not None:
            sources["queue"] = "queue-depth.csv"
        artifact["sources"] = sources
        artifact["estimators"] = {
            "cpu_percent": "process CPU tick delta divided by resource-sample timestamp duration",
            "rss_bytes": "maximum observed RSS",
            "pmic_internal_rail_proxy_watts": "arithmetic mean of valid PMIC internal-rail samples",
            "temperature_millicelsius": "maximum observed temperature",
            "queue_depth": "maximum observed queue depth",
        }
        artifact["rows"] = composed_rows
        destination = parent / "interval-metrics.json"
        with destination.open("x") as stream:
            json.dump(artifact, stream, indent=2)
            stream.write("\n")
        validate_interval_metrics(destination, aggregate_count)
        artifacts.append(destination)
    return artifacts[0] if len(artifacts) == 1 else output


def validate_interval_metrics(path: Path, aggregate_count: int | None = None) -> None:
    value = _load_json(path)
    required_top_level = {
        "schema_version",
        "interval_clock",
        "alignment_clock",
        "alignment_clock_purpose",
        "measurement_start_unix_epoch_ns",
        "declared_measurement_duration_ns",
        "bucket_width_ns",
        "maximum_rows",
        "row_count",
        "aggregate_latency_count",
        "late_arrivals",
        "sample_unit",
        "sources",
        "estimators",
        "rows",
    }
    if required_top_level - value.keys():
        raise ValueError("interval-metrics is missing required fields")
    if (
        value.get("schema_version") != 1
        or value.get("interval_clock") != "monotonic-elapsed"
        or value.get("alignment_clock") != "unix-epoch"
        or value.get("alignment_clock_purpose") != "cross-process-alignment-only"
        or value.get("bucket_width_ns") != INTERVAL_WIDTH_NS
        or value.get("sample_unit") != "interval-within-run"
    ):
        raise ValueError("interval-metrics has invalid schema or clock labels")
    duration_ns = int(value.get("declared_measurement_duration_ns", -1))
    maximum_rows = int(value.get("maximum_rows", -1))
    if maximum_rows != math.ceil(duration_ns / INTERVAL_WIDTH_NS) + 2:
        raise ValueError("interval-metrics maximum rows differs from declared duration")
    if int(value.get("late_arrivals", -1)) != 0:
        raise ValueError("interval-metrics contains late arrivals")
    rows = value.get("rows")
    if not isinstance(rows, list) or value.get("row_count") != len(rows):
        raise ValueError("interval-metrics row count differs")
    if duration_ns > 0 and not rows:
        raise ValueError("interval-metrics has no interval rows")
    if len(rows) > maximum_rows:
        raise ValueError("interval-metrics exceeds maximum rows")
    serialized = json.dumps(value)
    for field in FORBIDDEN_FIELDS:
        if f'"{field}"' in serialized:
            raise ValueError(f"interval-metrics contains forbidden field {field}")
    declared_count = int(value.get("aggregate_latency_count", -1))
    if aggregate_count is None:
        aggregate_count = declared_count
    elif declared_count != aggregate_count:
        raise ValueError("interval-metrics aggregate declaration differs")
    previous_end = 0
    total = 0
    required_row_fields = {
        "interval_start_ns",
        "interval_end_ns",
        "interval_start_unix_epoch_ns",
        "interval_end_unix_epoch_ns",
        "latency_count",
        "latency_p50_ns",
        "latency_p95_ns",
        "latency_p99_ns",
        "received_events",
        "throughput_messages",
        "throughput_messages_per_second",
        "duplicates",
        "cpu_percent",
        "rss_bytes",
        "pmic_internal_rail_proxy_watts",
        "temperature_millicelsius",
        "queue_depth",
    }
    metric_fields = (
        "latency_p50_ns",
        "latency_p95_ns",
        "latency_p99_ns",
        "cpu_percent",
        "rss_bytes",
        "pmic_internal_rail_proxy_watts",
        "temperature_millicelsius",
        "queue_depth",
    )
    for index, row in enumerate(rows):
        if not isinstance(row, dict) or required_row_fields - row.keys():
            raise ValueError("interval-metrics row is missing required fields")
        start = int(row["interval_start_ns"])
        end = int(row["interval_end_ns"])
        width = end - start
        if (
            start != previous_end
            or width <= 0
            or width > INTERVAL_WIDTH_NS
            or (index < len(rows) - 1 and width != INTERVAL_WIDTH_NS)
        ):
            raise ValueError("interval-metrics rows have gaps, overlap, or invalid widths")
        expected_start_unix = int(value["measurement_start_unix_epoch_ns"]) + start
        expected_end_unix = int(value["measurement_start_unix_epoch_ns"]) + end
        if (
            int(row["interval_start_unix_epoch_ns"]) != expected_start_unix
            or int(row["interval_end_unix_epoch_ns"]) != expected_end_unix
        ):
            raise ValueError("interval-metrics Unix alignment differs from monotonic bounds")
        if int(row["received_events"]) != int(row["throughput_messages"]) + int(
            row["duplicates"]
        ):
            raise ValueError("interval-metrics populations do not reconcile")
        for field in metric_fields:
            metric = row.get(field)
            if not isinstance(metric, dict) or metric.get("status") not in {"available", "unavailable"}:
                raise ValueError(f"interval-metrics {field} must declare availability")
            if metric["status"] == "available" and "value" not in metric:
                raise ValueError(f"interval-metrics {field} lacks a value")
            if metric["status"] == "unavailable" and not metric.get("reason"):
                raise ValueError(f"interval-metrics {field} lacks an unavailable reason")
        total += int(row["latency_count"])
        previous_end = end
    if aggregate_count is not None and total != aggregate_count:
        raise ValueError(
            f"interval-metrics latency population {total} differs from aggregate {aggregate_count}"
        )


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("output", type=Path)
    parser.add_argument("--required", action="store_true")
    args = parser.parse_args()
    try:
        result = compose_interval_metrics(args.output, required=args.required)
    except (OSError, ValueError, KeyError, TypeError) as error:
        print(f"error: {error}", file=sys.stderr)
        return 1
    if result is None:
        print("interval metrics: not applicable")
    else:
        print(f"interval metrics: {result}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
