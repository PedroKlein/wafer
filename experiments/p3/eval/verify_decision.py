#!/usr/bin/env python3
import argparse
import hashlib
import json
import statistics
from collections import defaultdict
from pathlib import Path

from jsonschema import Draft202012Validator

P3 = Path(__file__).resolve().parents[1]


def digest(path):
    return hashlib.sha256(Path(path).read_bytes()).hexdigest()


def scalar(rows, section, key, reduction=statistics.median):
    return reduction(row[section][key] for row in rows)


def aggregate(rows):
    return {
        "throughput_median_messages_per_second": scalar(rows, "steady_state", "throughput_messages_per_second"),
        "p50_median_ns": scalar(rows, "steady_state", "latency_p50_ns"),
        "p95_median_ns": scalar(rows, "steady_state", "latency_p95_ns"),
        "p99_median_ns": scalar(rows, "steady_state", "latency_p99_ns"),
        "peak_rss_max_bytes": scalar(rows, "steady_state", "peak_rss_bytes", max),
        "compile_median_ns": scalar(rows, "setup", "compile_ns"),
        "instantiate_median_ns": scalar(rows, "setup", "instantiate_ns"),
        "lost_messages": sum(row["steady_state"]["lost_messages"] for row in rows),
        "duplicate_messages": sum(row["steady_state"]["duplicate_messages"] for row in rows),
        "payload_copy_bytes": sum(row["copy_accounting"]["payload_copy_bytes"] for row in rows),
        "payload_allocations": sum(row["copy_accounting"]["payload_allocations"] for row in rows),
    }


def compare(candidate, baseline, benefit):
    throughput = 100 * (candidate["throughput_median_messages_per_second"] - baseline["throughput_median_messages_per_second"]) / baseline["throughput_median_messages_per_second"]
    p95 = 100 * (candidate["p95_median_ns"] - baseline["p95_median_ns"]) / baseline["p95_median_ns"]
    rss = candidate["peak_rss_max_bytes"] - baseline["peak_rss_max_bytes"]
    allowance = max(0.05 * baseline["peak_rss_max_bytes"], 2 * 1024 * 1024)
    return {
        "throughput_delta_pct": throughput,
        "p95_delta_pct": p95,
        "rss_delta_bytes": rss,
        "rss_allowance_bytes": allowance,
        "regression_budget_pass": throughput >= -5 and p95 <= 5 and rss <= allowance,
        "stream_benefit_pass": (throughput >= 10 or p95 <= -10) if benefit else False,
    }


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--root", required=True, type=Path)
    parser.add_argument("--decision", required=True, type=Path)
    args = parser.parse_args()

    decision = json.loads(args.decision.read_text())
    schema = json.loads((P3 / "decision-schema.json").read_text())
    Draft202012Validator(schema).validate(decision)
    leaves = [json.loads(path.read_text()) for path in sorted(args.root.glob("raw/**/result.json"))]
    assert len(leaves) == 108
    grouped = defaultdict(list)
    for leaf in leaves:
        assert leaf["status"] == "passed"
        assert leaf["source"]["git_sha"] == decision["source"]["git_sha"]
        assert leaf["correctness"]["input_messages"] == leaf["correctness"]["output_messages"]
        assert leaf["correctness"]["ordered"] and leaf["correctness"]["field_exact"]
        assert leaf["steady_state"]["lost_messages"] == 0
        assert leaf["steady_state"]["duplicate_messages"] == 0
        assert leaf["copy_accounting"]["reconciled"]
        grouped[(leaf["condition"], leaf["arm"])].append(leaf)

    for condition in decision["conditions"]:
        name = condition["condition"]
        recalculated = {arm: aggregate(grouped[(name, arm)]) for arm in ["p2", "p3-message", "p3-stream"]}
        assert recalculated["p2"] == condition["p2"]
        assert recalculated["p3-message"] == condition["p3_message"]
        assert recalculated["p3-stream"] == condition["p3_stream"]
        assert compare(recalculated["p3-message"], recalculated["p2"], False) == condition["message_vs_p2"]
        assert compare(recalculated["p3-stream"], recalculated["p2"], name == "100kb-depth5") == condition["stream_vs_p2"]

    assert decision["gates"]["go-p3-toolchain"] == "fail"
    assert decision["gates"]["wasmtime-p3-production-readiness"] == "fail"
    assert decision["decision"] == "defer-p3-toolchain"
    assert decision["inputs"]["admitted_leaf_sha256"] == digest(args.root / "admitted-leaves.sha256")
    print("leaves=108")
    print("conditions=6")
    print("independent_recomputation=match")
    print("decision=defer-p3-toolchain")


if __name__ == "__main__":
    main()
