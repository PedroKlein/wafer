from __future__ import annotations

import re


SWAP5_EVENT_COUNT = 50
SWAP5_PLUGIN = "wafer_pass_through_v2_panics.wasm"
_SEQUENCE_FIELDS = {"expected", "received", "gaps", "duplicates"}


def _validate_sequence(sequence: dict) -> None:
    if set(sequence) != _SEQUENCE_FIELDS or any(
        type(sequence.get(field)) is not int or sequence[field] < 0
        for field in _SEQUENCE_FIELDS
    ):
        raise ValueError("E-Swap-5 sequence evidence is malformed")


def _rollback_events(requests: list[dict]) -> list[dict]:
    if len(requests) != SWAP5_EVENT_COUNT:
        raise ValueError("E-Swap-5 requires exactly 50 rollback requests")
    events = []
    for index, request in enumerate(requests):
        body = request.get("body")
        timeline = body.get("timeline") if isinstance(body, dict) else None
        if (
            request.get("event_index") != index
            or request.get("plugin") != SWAP5_PLUGIN
            or request.get("http_status") != 200
            or not isinstance(body, dict)
            or body.get("status") != "rolled_back"
            or not isinstance(timeline, dict)
        ):
            raise ValueError(f"E-Swap-5 rollback request {index} is inconsistent")
        event = {"event_index": index}
        for field in ("compile_ns", "instantiate_ns", "signal_ns", "rollback_ns"):
            value = timeline.get(field)
            if type(value) is not int or value < 0:
                raise ValueError(f"E-Swap-5 rollback request {index} has invalid {field}")
            event[field] = value
        events.append(event)
    return events


def build_swap5_rollback(requests: list[dict], sequence: dict) -> dict:
    _validate_sequence(sequence)
    events = _rollback_events(requests)
    return {
        "schema_version": 1,
        "duration_unit": "ns",
        "independent_unit": "complete process run",
        "nested_unit": "rollback event within run",
        "attempts": len(events),
        "rolled_back": len(events),
        "all_rolled_back": True,
        "sequence": dict(sequence),
        "events": events,
    }


def build_post_rollback_continuity(
    requests: list[dict],
    interval_metrics: dict,
    sequence: dict,
    *,
    interval_metrics_sha256: str,
) -> dict:
    _validate_sequence(sequence)
    _rollback_events(requests)
    final_finished = requests[-1].get("request_finished_ns")
    if type(final_finished) is not int or final_finished < 0:
        raise ValueError("E-Swap-5 final rollback completion is invalid")
    rows = interval_metrics.get("rows")
    if not isinstance(rows, list):
        raise ValueError("E-Swap-5 interval metrics are missing")
    after = [
        row
        for row in rows
        if type(row.get("interval_start_unix_epoch_ns")) is int
        and row["interval_start_unix_epoch_ns"] >= final_finished
    ]
    if not after:
        raise ValueError("E-Swap-5 has no post-rollback observation interval")
    messages = sum(
        row["throughput_messages"]
        for row in after
        if type(row.get("throughput_messages")) is int
        and row["throughput_messages"] >= 0
    )
    return {
        "schema_version": 1,
        "clock": "unix-epoch",
        "final_rollback_event_index": SWAP5_EVENT_COUNT - 1,
        "final_rollback_finished_ns": final_finished,
        "observation_start_ns": after[0]["interval_start_unix_epoch_ns"],
        "observation_end_ns": after[-1].get("interval_end_unix_epoch_ns"),
        "messages_after_final_rollback": messages,
        "output_observed_after_final_rollback": messages > 0,
        "successful_v2_transition_observed": False,
        "interval_metrics_path": "interval-metrics.json",
        "interval_metrics_sha256": interval_metrics_sha256,
        "sequence": dict(sequence),
    }


def validate_swap5_artifacts(
    requests: list[dict], rollback: dict, continuity: dict, sequence: dict
) -> None:
    _validate_sequence(sequence)
    events = _rollback_events(requests)
    if (
        rollback.get("schema_version") != 1
        or rollback.get("duration_unit") != "ns"
        or rollback.get("independent_unit") != "complete process run"
        or rollback.get("nested_unit") != "rollback event within run"
        or rollback.get("attempts") != SWAP5_EVENT_COUNT
        or rollback.get("rolled_back") != SWAP5_EVENT_COUNT
        or rollback.get("all_rolled_back") is not True
        or rollback.get("sequence") != sequence
        or rollback.get("events") != events
    ):
        raise ValueError("E-Swap-5 rollback evidence does not reconcile")
    final_finished = requests[-1].get("request_finished_ns")
    if continuity.get("successful_v2_transition_observed") is not False:
        raise ValueError("E-Swap-5 must not claim a successful v2 transition")
    digest = continuity.get("interval_metrics_sha256")
    if (
        continuity.get("schema_version") != 1
        or continuity.get("clock") != "unix-epoch"
        or continuity.get("final_rollback_event_index") != SWAP5_EVENT_COUNT - 1
        or continuity.get("final_rollback_finished_ns") != final_finished
        or type(continuity.get("observation_start_ns")) is not int
        or continuity["observation_start_ns"] < final_finished
        or type(continuity.get("observation_end_ns")) is not int
        or continuity["observation_end_ns"] <= continuity["observation_start_ns"]
        or type(continuity.get("messages_after_final_rollback")) is not int
        or continuity["messages_after_final_rollback"] <= 0
        or continuity.get("output_observed_after_final_rollback") is not True
        or continuity.get("interval_metrics_path") != "interval-metrics.json"
        or not isinstance(digest, str)
        or re.fullmatch(r"[0-9a-f]{64}", digest) is None
        or continuity.get("sequence") != sequence
    ):
        raise ValueError("E-Swap-5 post-rollback output evidence is invalid")
