from __future__ import annotations

BACKPRESSURE_POLICIES = ("slow", "drop", "dead-letter")
BACKPRESSURE_COUNT_FIELDS = {
    "attempted",
    "accepted",
    "processed",
    "delivered",
    "dropped",
    "dead_lettered",
    "downstream_closed",
    "dlq_full",
    "dlq_closed",
    "outstanding",
}


def _nonnegative_counts(result: dict) -> dict[str, int]:
    counts = result.get("counts")
    if not isinstance(counts, dict) or set(counts) != BACKPRESSURE_COUNT_FIELDS:
        raise ValueError("backpressure counts differ from the runtime counter schema")
    if any(isinstance(value, bool) or not isinstance(value, int) or value < 0 for value in counts.values()):
        raise ValueError("backpressure counts must be non-negative integers")
    return counts


def validate_backpressure_result(result: dict, expected_policy: str | None = None) -> None:
    policy = result.get("policy")
    if (
        result.get("schema_version") != 2
        or result.get("experiment") != "e-backpressure"
        or result.get("sample_unit") != "run"
        or policy not in BACKPRESSURE_POLICIES
        or result.get("condition") != policy
        or (expected_policy is not None and policy != expected_policy)
    ):
        raise ValueError("backpressure policy/schema differs from the declared condition")
    if result.get("queue") != "slow":
        raise ValueError("backpressure evidence targets the wrong queue")
    if result.get("classification") != "saturated-and-drained":
        raise ValueError("backpressure run did not cross and recover below queue thresholds")
    peak = result.get("peak_occupancy")
    threshold = result.get("occupancy_threshold")
    if (
        result.get("threshold_crossed") is not True
        or result.get("recovered") is not True
        or not isinstance(peak, (int, float))
        or not isinstance(threshold, (int, float))
        or peak < threshold
    ):
        raise ValueError("backpressure evidence did not cross and recover below thresholds")
    rates = result.get("rates_msg_s")
    if not isinstance(rates, dict) or set(rates) != {
        "offered",
        "accepted",
        "processed",
        "drained",
    }:
        raise ValueError("backpressure rates must include offered, accepted, processed, and drained")
    if rates["drained"] is None:
        raise ValueError("backpressure run has no measurable drain rate")

    counts = _nonnegative_counts(result)
    sequence = result.get("sequence")
    if not isinstance(sequence, dict) or set(sequence) != {
        "offered",
        "received",
        "gaps",
        "duplicates",
    }:
        raise ValueError("backpressure sequence accounting is malformed")
    if any(isinstance(value, bool) or not isinstance(value, int) or value < 0 for value in sequence.values()):
        raise ValueError("backpressure sequence counts must be non-negative integers")
    if (
        counts["attempted"] != sequence["offered"]
        or counts["delivered"] != sequence["received"]
        or counts["accepted"] != counts["processed"]
        or counts["processed"] != counts["delivered"]
        or counts["outstanding"] != counts["accepted"] - counts["processed"]
        or sequence["gaps"] != counts["attempted"] - counts["delivered"]
        or sequence["duplicates"] != 0
        or counts["downstream_closed"] != 0
    ):
        raise ValueError("backpressure runtime and sequence counters do not reconcile")

    accounting = result.get("accounting", {})
    equations = {
        "slow": "attempted = delivered",
        "drop": "attempted = delivered + dropped",
        "dead-letter": "attempted = delivered + dead_lettered + dlq_full + dlq_closed",
    }
    if accounting.get("equation") != equations[policy]:
        raise ValueError("backpressure accounting equation differs from the policy")
    failures = accounting.get("dlq_failures")
    if failures != {
        "full": counts["dlq_full"],
        "closed": counts["dlq_closed"],
        "total": counts["dlq_full"] + counts["dlq_closed"],
    } or accounting.get("reconciled") is not True:
        raise ValueError("backpressure DLQ failure accounting is hidden or inconsistent")

    if policy == "slow":
        if (
            counts["attempted"] != counts["delivered"]
            or any(counts[field] for field in ("dropped", "dead_lettered", "dlq_full", "dlq_closed"))
            or result.get("producer_progress") != "backpressured"
        ):
            raise ValueError("backpressure slow policy is not lossless")
    elif policy == "drop":
        if (
            counts["attempted"] != counts["delivered"] + counts["dropped"]
            or any(counts[field] for field in ("dead_lettered", "dlq_full", "dlq_closed"))
            or result.get("producer_progress") != "nonblocking"
        ):
            raise ValueError("backpressure drop policy accounting does not reconcile")
    elif (
        counts["attempted"]
        != counts["delivered"]
        + counts["dead_lettered"]
        + counts["dlq_full"]
        + counts["dlq_closed"]
        or counts["dropped"] != 0
        or result.get("producer_progress") != "nonblocking"
    ):
        raise ValueError("backpressure dead-letter policy accounting does not reconcile")

    if result.get("memory", {}).get("within_limit") is not True:
        raise ValueError("backpressure run exceeded the frozen RSS bound")
