#!/usr/bin/env python3
import argparse
import hashlib
import json
import math
import statistics
from pathlib import Path

from jsonschema import Draft202012Validator

P3 = Path(__file__).resolve().parents[1]


def sha256(path):
    return hashlib.sha256(Path(path).read_bytes()).hexdigest()


def median(rows, field):
    return statistics.median(row["steady_state"][field] for row in rows)


def aggregate(rows):
    return {
        "throughput_median_messages_per_second": median(rows, "throughput_messages_per_second"),
        "p50_median_ns": median(rows, "latency_p50_ns"),
        "p95_median_ns": median(rows, "latency_p95_ns"),
        "p99_median_ns": median(rows, "latency_p99_ns"),
        "peak_rss_max_bytes": max(row["steady_state"]["peak_rss_bytes"] for row in rows),
        "compile_median_ns": median(rows, "compile_ns") if "compile_ns" in rows[0] else statistics.median(row["setup"]["compile_ns"] for row in rows),
        "instantiate_median_ns": statistics.median(row["setup"]["instantiate_ns"] for row in rows),
        "lost_messages": sum(row["steady_state"]["lost_messages"] for row in rows),
        "duplicate_messages": sum(row["steady_state"]["duplicate_messages"] for row in rows),
        "payload_copy_bytes": sum(row["copy_accounting"]["payload_copy_bytes"] for row in rows),
        "payload_allocations": sum(row["copy_accounting"]["payload_allocations"] for row in rows),
    }


def delta(candidate, baseline, benefit=False):
    throughput = 100 * (
        candidate["throughput_median_messages_per_second"]
        - baseline["throughput_median_messages_per_second"]
    ) / baseline["throughput_median_messages_per_second"]
    p95 = 100 * (candidate["p95_median_ns"] - baseline["p95_median_ns"]) / baseline["p95_median_ns"]
    rss = candidate["peak_rss_max_bytes"] - baseline["peak_rss_max_bytes"]
    allowance = max(0.05 * baseline["peak_rss_max_bytes"], 2 * 1024 * 1024)
    return {
        "throughput_delta_pct": throughput,
        "p95_delta_pct": p95,
        "rss_delta_bytes": rss,
        "rss_allowance_bytes": allowance,
        "regression_budget_pass": throughput >= -5.0 and p95 <= 5.0 and rss <= allowance,
        "stream_benefit_pass": throughput >= 10.0 or -p95 >= 10.0 if benefit else False,
    }


def decide(gates):
    toolchain_names = {
        "rust-p3-toolchain",
        "go-p3-toolchain",
        "wasmtime-p3-production-readiness",
    }
    if any(gates[name] != "pass" for name in toolchain_names):
        return "defer-p3-toolchain"
    if all(value == "pass" for name, value in gates.items() if name not in toolchain_names):
        return "migrate-wit-before-release"
    return "retain-p2-for-v1"


def validate_leaf(leaf, row, position, batch, contract, schema):
    Draft202012Validator(schema).validate(leaf)
    controls = contract["controlled_factors"]
    assert leaf["source"] == {"git_sha": batch["source"]["git_sha"], "git_dirty": False, "evidence_class": "experimental-diagnostic"}
    assert leaf["condition"] == row["condition"]
    assert leaf["arm"] == row["arm_order"][position - 1]
    assert leaf["triplet_index"] == row["replicate"]
    assert leaf["position"] == position
    assert leaf["controlled_factors"] == {
        "payload_bytes": row["payload_bytes"],
        "depth": row["depth"],
        "queue_capacity_elements": controls["queue_capacity_elements"],
        "stream_session_capacity_elements": controls["stream_session_capacity_elements"],
        "worker_threads": controls["worker_threads"],
        "warmup_messages": controls["warmup_messages"],
        "measured_messages": controls["measured_messages"],
        "release": controls["release"],
        "locked": controls["locked"],
        "metering_mode": controls["metering_mode"],
        "outer_timeout_seconds": controls["outer_timeout_seconds"],
    }
    assert leaf["hashes"]["executable"] == batch["host_sha256"]
    assert leaf["hashes"]["component"] == batch["components"][leaf["arm"]]
    assert leaf["hashes"]["wit_tree"] == batch["wit_tree_sha256"]
    assert leaf["hashes"]["experiment_contract"] == batch["contract_sha256"]
    steady = leaf["steady_state"]
    correctness = leaf["correctness"]
    copies = leaf["copy_accounting"]
    measured = controls["measured_messages"]
    assert correctness == {
        "input_messages": measured,
        "output_messages": measured,
        "ordered": True,
        "field_exact": True,
        "session_completed": True,
    }
    assert steady["lost_messages"] == 0 and steady["duplicate_messages"] == 0
    assert math.isfinite(steady["throughput_messages_per_second"])
    assert steady["latency_p50_ns"] <= steady["latency_p95_ns"] <= steady["latency_p99_ns"]
    expected_bytes = measured * row["depth"] * row["payload_bytes"] * 2
    expected_allocations = measured * row["depth"] * 2
    assert copies == {
        "payload_copy_bytes": expected_bytes,
        "payload_allocations": expected_allocations,
        "reconciled": True,
    }


def select_leaves(root, schedule, batch, contract, schema):
    selected = []
    for row in schedule["triplets"]:
        for position, arm in enumerate(row["arm_order"], 1):
            slot = root / "raw" / row["condition"] / f"triplet-{row['replicate']:02}" / f"{position}-{arm}"
            passed = []
            for result_path in sorted(slot.glob("attempt-*/result.json")):
                leaf = json.loads(result_path.read_text())
                if leaf.get("status") == "passed":
                    validate_leaf(leaf, row, position, batch, contract, schema)
                    passed.append((result_path, leaf))
            assert len(passed) == 1, f"{slot}: expected exactly one passed attempt, found {len(passed)}"
            selected.append(passed[0])
    assert len(selected) == schedule["run_count"]
    return selected


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--root", required=True, type=Path)
    parser.add_argument("--output", required=True, type=Path)
    parser.add_argument("--check", type=Path)
    args = parser.parse_args()

    root = args.root.resolve()
    contract = json.loads((P3 / "experiment-contract.json").read_text())
    schedule = json.loads((P3 / "dry-run-schedule.json").read_text())
    schema = json.loads((P3 / "raw-leaf-schema.json").read_text())
    decision_schema = json.loads((P3 / "decision-schema.json").read_text())
    batch = json.loads((root / "batch.json").read_text())
    assert batch["contract_sha256"] == sha256(P3 / "experiment-contract.json")
    assert batch["schedule_sha256"] == sha256(P3 / "dry-run-schedule.json")

    selected = select_leaves(root, schedule, batch, contract, schema)
    index_text = "".join(f"{sha256(path)}  {path.relative_to(root)}\n" for path, _ in selected)
    assert (root / "admitted-leaves.sha256").read_text() == index_text

    by_condition = []
    all_message_regressions = True
    all_stream_regressions = True
    high_pressure_benefit = False
    for condition in contract["primary_conditions"]:
        arms = {}
        for arm in contract["arms"]:
            rows = [leaf for _, leaf in selected if leaf["condition"] == condition and leaf["arm"] == arm]
            assert len(rows) == contract["triplets_per_condition"]
            arms[arm] = aggregate(rows)
        message_delta = delta(arms["p3-message"], arms["p2"])
        stream_delta = delta(
            arms["p3-stream"],
            arms["p2"],
            benefit=condition in contract["high_pressure_conditions"],
        )
        all_message_regressions &= message_delta["regression_budget_pass"]
        all_stream_regressions &= stream_delta["regression_budget_pass"]
        if condition in contract["high_pressure_conditions"]:
            high_pressure_benefit |= stream_delta["stream_benefit_pass"]
        by_condition.append({
            "condition": condition,
            "admitted_triplets": contract["triplets_per_condition"],
            "p2": arms["p2"],
            "p3_message": arms["p3-message"],
            "p3_stream": arms["p3-stream"],
            "message_vs_p2": message_delta,
            "stream_vs_p2": stream_delta,
        })

    go_verdict = json.loads((P3 / "go/verdict.json").read_text())
    toolchain_manifest = json.loads((P3 / "toolchain-manifest.json").read_text())
    gates = {
        "source-and-artifact-provenance": "pass",
        "production-p2-compatibility": "pass",
        "rust-p3-toolchain": "pass",
        "go-p3-toolchain": "pass" if go_verdict["adoption_consequence"] == "go-p3-ready" else "fail",
        "wasmtime-p3-production-readiness": "pass" if toolchain_manifest["production_readiness"] == "production-ready" else "fail",
        "lifecycle-conformance": "pass",
        "lossless-ordering": "pass",
        "copy-allocation-reconciliation": "pass",
        "complete-matched-triplets": "pass",
        "p3-message-regression": "pass" if all_message_regressions else "fail",
        "p3-stream-regression": "pass" if all_stream_regressions else "fail",
        "p3-stream-benefit": "pass" if high_pressure_benefit else "fail",
    }
    decision = decide(gates)
    if decision == "defer-p3-toolchain":
        revisit = "Re-run after Wasmtime P3 is production-ready and the maintained Go toolchain builds, validates, and executes wafer:pipeline@0.2.0 async message and stream worlds."
    elif decision == "retain-p2-for-v1":
        revisit = "Revisit when a versioned P3 candidate passes every frozen semantic and performance gate."
    else:
        revisit = None

    output = {
        "schema": "wafer-p3-poc-decision-v1",
        "source": {
            "git_sha": batch["source"]["git_sha"],
            "git_dirty": False,
            "evidence_class": "experimental-diagnostic",
            "thesis_evidence": False,
        },
        "inputs": {
            "contract_sha256": batch["contract_sha256"],
            "schedule_sha256": batch["schedule_sha256"],
            "admitted_leaf_sha256": sha256(root / "admitted-leaves.sha256"),
        },
        "gates": gates,
        "conditions": by_condition,
        "decision": decision,
        "revisit_trigger": revisit,
    }
    Draft202012Validator(decision_schema).validate(output)
    serialized = json.dumps(output, indent=2, sort_keys=True) + "\n"
    if args.check:
        assert json.loads(args.check.read_text()) == output
        print("independent_recomputation=match")
    args.output.write_text(serialized)
    print(f"admitted_leaves={len(selected)}")
    print(f"decision={decision}")


if __name__ == "__main__":
    main()
