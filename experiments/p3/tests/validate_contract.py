#!/usr/bin/env python3
import itertools
import json
from collections import Counter
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
REPO = ROOT.parents[1]

contract = json.loads((ROOT / "experiment-contract.json").read_text())
schedule = json.loads((ROOT / "dry-run-schedule.json").read_text())
raw_schema = json.loads((ROOT / "raw-leaf-schema.json").read_text())
decision_schema = json.loads((ROOT / "decision-schema.json").read_text())
adr = (REPO / "docs/adr/0017-experimental-wasip3-streaming.md").read_text()

assert contract["schema"] == "wafer-p3-poc-experiment-v1"
assert contract["arms"] == ["p2", "p3-message", "p3-stream"]
assert [item["bytes"] for item in contract["payloads"]] == [120, 1024, 102400]
assert contract["depths"] == [1, 5]
assert len(contract["primary_conditions"]) == 6
assert contract["high_pressure_conditions"] == ["100kb-depth5"]
assert contract["triplets_per_condition"] >= 6

orders = contract["triplet_orders"]
assert len(orders) == contract["triplets_per_condition"]
for order in orders:
    assert sorted(order) == sorted(contract["arms"])
for arm in contract["arms"]:
    positions = Counter(order.index(arm) + 1 for order in orders)
    assert positions == {1: 2, 2: 2, 3: 2}

assert schedule["triplet_count"] == 36
assert schedule["run_count"] == 108
assert len(schedule["triplets"]) == schedule["triplet_count"]
for row in schedule["triplets"]:
    assert row["arm_order"] == orders[row["replicate"] - 1]
    assert row["condition"] in contract["primary_conditions"]

controls = contract["controlled_factors"]
assert controls["queue_capacity_elements"] == 32
assert controls["stream_session_capacity_elements"] == 32
assert controls["metering_mode"] == "neither"
assert controls["fuel"] is None
assert controls["epoch_deadline_ticks"] is None

message_gate = contract["gates"]["p3_message_vs_p2"]
stream_gate = contract["gates"]["p3_stream_vs_p2"]
assert message_gate["minimum_throughput_delta_pct"] == -5.0
assert message_gate["maximum_p95_delta_pct"] == 5.0
assert stream_gate["minimum_throughput_delta_pct"] == -5.0
assert stream_gate["maximum_p95_delta_pct"] == 5.0
assert stream_gate["benefit"] == {
    "condition": "100kb-depth5",
    "minimum_throughput_improvement_pct": 10.0,
    "minimum_p95_improvement_pct": 10.0,
    "operator": "or",
}


def decide(toolchain_pass, all_other_gates_pass):
    if not toolchain_pass:
        return "defer-p3-toolchain"
    if all_other_gates_pass:
        return "migrate-wit-before-release"
    return "retain-p2-for-v1"


allowed = {
    "defer-p3-toolchain",
    "migrate-wit-before-release",
    "retain-p2-for-v1",
}
for toolchain_pass, all_other_gates_pass in itertools.product([False, True], repeat=2):
    outcome = decide(toolchain_pass, all_other_gates_pass)
    assert outcome in allowed
    assert sum(outcome == candidate for candidate in allowed) == 1


def regression_pass(throughput_delta_pct, p95_delta_pct, rss_delta_bytes, p2_rss_bytes):
    allowance = max(0.05 * p2_rss_bytes, 2 * 1024 * 1024)
    return throughput_delta_pct >= -5.0 and p95_delta_pct <= 5.0 and rss_delta_bytes <= allowance


assert regression_pass(-5.0, 5.0, 2 * 1024 * 1024, 16 * 1024 * 1024)
assert not regression_pass(-5.01, 5.0, 0, 16 * 1024 * 1024)
assert not regression_pass(-5.0, 5.01, 0, 16 * 1024 * 1024)
assert not regression_pass(-5.0, 5.0, 2 * 1024 * 1024 + 1, 16 * 1024 * 1024)
assert 10.0 >= stream_gate["benefit"]["minimum_throughput_improvement_pct"]
assert -(-10.0) >= stream_gate["benefit"]["minimum_p95_improvement_pct"]

raw_required = set(raw_schema["required"])
assert {"source", "hashes", "setup", "steady_state", "correctness", "copy_accounting", "status"} <= raw_required
assert raw_schema["properties"]["source"]["properties"]["git_dirty"]["const"] is False
assert raw_schema["properties"]["status"]["enum"] == ["passed", "failed"]
assert set(decision_schema["properties"]["decision"]["enum"]) == allowed
assert decision_schema["properties"]["source"]["properties"]["thesis_evidence"]["const"] is False

for test_id in [*(f"C{i:02}" for i in range(1, 15)), *(f"E{i:02}" for i in range(1, 5))]:
    assert f"`{test_id}`" in adr
for forbidden in ["TODO", "TBD"]:
    assert forbidden not in adr

print("conditions=6")
print("triplets=36")
print("leaves=108")
print("balanced_positions=true")
print("lifecycle_tests=14")
print("experiment_tests=4")
print("decision_outcomes=3-exclusive")
