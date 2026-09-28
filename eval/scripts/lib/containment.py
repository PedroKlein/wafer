"""Containment verdict for the isolation experiments.

Each attack must be stopped by one specific mechanism, read from the attack
node's row in per_node_metrics.csv. Any other failure on that node, or any
message it passed on, means the attack was not stopped the way it claims.
"""

from __future__ import annotations

EXPECTED_MECHANISM: dict[tuple[str, str], tuple[str, str]] = {
    ("e-iso-1", "buffer-overflow"): ("attack", "traps_memory_out_of_bounds"),
    ("e-iso-2", "cross-read"): ("attack", "traps_memory_out_of_bounds"),
    # No preopen is granted, so WASI refuses the read inside the guest and
    # never reaches the host; the plugin reports it as `unrecoverable`.
    ("e-iso-3", "fs-access"): ("attack", "guest_unrecoverable"),
    ("e-iso-4", "infinite-loop"): ("attack", "traps_interrupt"),
    ("e-iso-5", "memory-exhaust"): ("attack", "traps_memory_limit"),
    ("e-iso-6", "panic"): ("attack", "traps_unreachable"),
    ("e-iso-7", "panic-attack"): ("branch_b", "traps_unreachable"),
    ("e-iso-7", "epoch-loop-attack"): ("branch_b", "traps_interrupt"),
    ("e-iso-8", "panic-recovery"): ("attack", "traps_unreachable"),
}


def assess_containment(rows: list[dict], experiment: str, condition: str) -> dict:
    """Judge one run from its per-node metric rows.

    Raises ValueError when the rows lack the attack node or its counters.
    """
    expectation = EXPECTED_MECHANISM.get((experiment, condition))
    if expectation is None:
        return {"attack_node": None, "expected_mechanism": None, "contained_by_mechanism": None}
    node_id, column = expectation
    row = next((row for row in rows if row.get("node_id") == node_id), None)
    if row is None:
        raise ValueError(f"per_node_metrics.csv has no {node_id} row")
    try:
        expected = int(row[column])
        failed = int(row["attempts_failed"])
        forwarded = int(row["messages_out"])
    except (KeyError, TypeError, ValueError) as error:
        raise ValueError(f"{node_id} row lacks containment counters") from error
    unexpected = failed - expected + forwarded
    return {
        "attack_node": node_id,
        "expected_mechanism": column,
        "expected_count": expected,
        "unexpected_outcomes": unexpected,
        "contained_by_mechanism": expected > 0 and unexpected == 0,
    }
