#!/usr/bin/env python3

from __future__ import annotations

import argparse
import hashlib
import json
import sys
from pathlib import Path

CANDIDATE_IDS = {
    "e-perf-capacity-knee",
    "e-perf-payload-refinement",
    "e-perf-depth-extension",
    "e-swap-independent-sessions",
    "e-swap-rollback-sessions",
    "e-host-thermal-storage",
    "e-compare-ekuiper-profile",
}
CAPABILITIES = {
    "bounded-interval-metrics",
    "multi-resolution-swap-buckets",
    "capacity-knee",
    "payload-refinement",
    "depth-extension",
    "independent-swap-sessions",
    "independent-rollback-sessions",
    "thermal-storage-characterization",
    "ekuiper-tail-profiling",
}
SAMPLE_UNITS = {
    "e-perf-capacity-knee": "independent host run at one system and offered rate",
    "e-perf-payload-refinement": "independent host run at one payload size",
    "e-perf-depth-extension": "independent host run at one pipeline depth",
    "e-swap-independent-sessions": "independent host run",
    "e-swap-rollback-sessions": "independent host run",
    "e-host-thermal-storage": "clean-boot host characterization session",
    "e-compare-ekuiper-profile": "independent host run at one rate and profiler state",
}
REQUIRED_OUTPUTS = {
    "e-perf-capacity-knee": {"capacity-run.json", "latency.hdr", "publisher-summary.json", "subscriber-metadata.json", "resource-usage.csv", "process-audit.json", "interval-metrics.json"},
    "e-perf-payload-refinement": {"latency.hdr", "throughput.csv", "sequence.csv", "interval-metrics.json", "payload-manifest.json"},
    "e-perf-depth-extension": {"latency.hdr", "throughput.csv", "sequence.csv", "memory.csv", "interval-metrics.json", "topology-manifest.json"},
    "e-swap-independent-sessions": {"latency.hdr", "throughput.csv", "sequence.csv", "swap_requests.json", "swap_timeline.json", "hotswap-analysis.json", "interval-metrics.json"},
    "e-swap-rollback-sessions": {"latency.hdr", "throughput.csv", "sequence.csv", "swap_requests.json", "rollback.json", "interval-metrics.json"},
    "e-host-thermal-storage": {"host-load-ladder.json", "host-telemetry.csv", "kernel-io.log", "usb-integrity.json"},
    "e-compare-ekuiper-profile": {"latency.hdr", "throughput.csv", "interval-metrics.json", "ekuiper-runtime-summary.json", "profiler-overhead.json"},
}
CONTRACT_PHRASES = (
    "candidate-supplementary",
    "post-rehearsal selection receipt",
    "PMIC internal-rail proxy",
    "not total input power",
    "E-Perf-5 remains PENDING",
    "no cross-architecture claim is made",
    "mandatory per-message traces remain forbidden",
    "/mnt/wafer-results",
    "/Volumes/WAF_RESULTS",
    "append-only",
)


def load(path: Path) -> dict:
    try:
        value = json.loads(path.read_text())
    except (OSError, json.JSONDecodeError) as error:
        raise ValueError(f"cannot load {path}: {error}") from error
    if not isinstance(value, dict):
        raise ValueError(f"{path} must contain a JSON object")
    return value


def validate(matrix: dict, decision: dict, contract: str) -> list[str]:
    errors: list[str] = []
    enhanced = matrix.get("enhanced_candidate")
    if not isinstance(enhanced, dict):
        return ["enhanced_candidate must be an object"]

    if (
        enhanced.get("release_lineage") != "v8"
        or enhanced.get("signed_v4_immutable") is not True
        or enhanced.get("signed_v5_immutable") is not True
        or enhanced.get("signed_v6_immutable") is not True
        or enhanced.get("signed_v7_immutable") is not True
        or enhanced.get("supersedes_release_tag") != "rpi5-final-rc-v7"
        or enhanced.get("supersession_reason")
        != "v7 eKuiper diagnostic validation rejects a contract-valid terminal partial interval"
    ):
        errors.append("enhanced architecture must preserve v4/v5/v6/v7 and use the corrective v8 lineage")
    if enhanced.get("thesis_evidence") is not False or enhanced.get("n30_admitted") is not False or enhanced.get("campaign_started") is not False:
        errors.append("enhanced candidate suite must remain diagnostic and outside N=30")
    if enhanced.get("selection_receipt_required") != ".plans/rpi5-v5-enhanced-experiment-readiness/n30-selection.json":
        errors.append("enhanced candidate suite must require the post-rehearsal selection receipt")

    capabilities = set(enhanced.get("capabilities", []))
    for capability in sorted(CAPABILITIES - capabilities):
        errors.append(f"required capability {capability} is missing")

    candidates = enhanced.get("experiments")
    if not isinstance(candidates, dict):
        return errors + ["enhanced_candidate experiments must be an object"]
    if set(candidates) != CANDIDATE_IDS:
        errors.append("candidate experiment IDs differ from the frozen set")

    for experiment_id in sorted(CANDIDATE_IDS):
        definition = candidates.get(experiment_id)
        if not isinstance(definition, dict):
            continue
        if definition.get("evidence_class") not in {"candidate-supplementary", "diagnostic"}:
            errors.append(f"candidate {experiment_id} evidence_class is invalid")
        if definition.get("thesis_evidence") is not False:
            errors.append(f"candidate {experiment_id} thesis_evidence must be false")
        if definition.get("n30_admitted") is not False:
            errors.append(f"candidate {experiment_id} n30_admitted must be false")
        if definition.get("sample_unit") != SAMPLE_UNITS[experiment_id]:
            errors.append(f"candidate {experiment_id} sample_unit differs from the frozen contract")
        if set(definition.get("required_outputs", [])) != REQUIRED_OUTPUTS[experiment_id]:
            errors.append(f"candidate {experiment_id} required_outputs differ from the frozen contract")
        if not definition.get("no_pool_with"):
            errors.append(f"candidate {experiment_id} must declare no_pool_with")

    capacity = candidates.get("e-perf-capacity-knee", {})
    expected_capacity = {
        "mqtt-loopback": [*range(4000, 16000, 1000), 15250, 15500, 15750, 16000],
        "native": list(range(8000, 16000, 1000)),
        "wafer": list(range(8000, 16000, 1000)),
        "ekuiper": list(range(4000, 9000, 1000)),
    }
    if capacity.get("condition_grid_msg_s") != expected_capacity or capacity.get("repetitions") != 5:
        errors.append("capacity-knee condition grid differs from the frozen contract")
    if {
        key: capacity.get(key)
        for key in ("warmup_secs", "measurement_secs", "loadgen_profile", "ordering", "delivery_good")
    } != {
        "warmup_secs": 30,
        "measurement_secs": 60,
        "loadgen_profile": "eval/loadgen/capacity-knee.toml",
        "ordering": {
            "method": "seeded rate blocks with five-run balanced system order",
            "default_seed": 1729,
            "cooldown_secs": 60,
        },
        "delivery_good": {
            "loss_aggregation": "sum(total_undelivered) / sum(intended)",
            "max_loss_percent": 1.0,
            "achieved_aggregation": "mean(run achieved_rate / intended_rate)",
            "min_achieved_ratio": 0.99,
            "duplicates_allowed": 0,
            "support_path_censoring": "mqtt-loopback",
        },
    }:
        errors.append("capacity-knee execution or estimator contract differs from the frozen primary")
    payload = candidates.get("e-perf-payload-refinement", {})
    if {
        key: payload.get(key)
        for key in (
            "payload_bytes",
            "payload_sha256",
            "repetitions",
            "warmup_secs",
            "measurement_secs",
            "rate_msg_s",
            "execution_path",
            "payload_pattern",
            "ordering",
            "no_pool_with",
        )
    } != {
        "payload_bytes": [120, 1024, 8192, 10240, 16384, 32768, 65536, 102400, 131072, 262144],
        "payload_sha256": {
            label: hashlib.sha256(b"B" * size).hexdigest()
            for label, size in (
                ("120b", 120), ("1kb", 1024), ("8kb", 8192), ("10kb", 10240),
                ("16kb", 16384), ("32kb", 32768), ("64kb", 65536),
                ("100kb", 102400), ("128kb", 131072), ("256kb", 262144),
            )
        },
        "repetitions": 5,
        "warmup_secs": 30,
        "measurement_secs": 60,
        "rate_msg_s": 1000,
        "execution_path": "in-process bench-source -> pass-through -> bench-sink",
        "payload_pattern": "repeated-byte-0x42",
        "ordering": {
            "method": "seeded condition shuffle per repetition",
            "default_seed": 1729,
        },
        "no_pool_with": ["e-perf-4", "prior diagnostic rehearsals"],
    }:
        errors.append("payload-refinement condition grid differs from the frozen contract")
    depth = candidates.get("e-perf-depth-extension", {})
    if {
        key: depth.get(key)
        for key in (
            "depths",
            "repetitions",
            "warmup_secs",
            "measurement_secs",
            "rate_msg_s",
            "payload_bytes",
            "execution_path",
            "ordering",
            "no_pool_with",
        )
    } != {
        "depths": [1, 3, 5, 10, 20, 50],
        "repetitions": 5,
        "warmup_secs": 30,
        "measurement_secs": 60,
        "rate_msg_s": 1000,
        "payload_bytes": 128,
        "execution_path": "in-process bench-source -> identical pass-through chain -> bench-sink",
        "ordering": {
            "method": "seeded condition shuffle per repetition",
            "default_seed": 1729,
        },
        "no_pool_with": [
            "e-perf-3",
            "e-perf-6",
            "e-perf-8",
            "prior diagnostic rehearsals",
        ],
    }:
        errors.append("depth-extension condition grid differs from the frozen contract")
    swap_conditions = {
        "e-swap-independent-sessions": ["steady"],
        "e-swap-rollback-sessions": ["process-trap-rollback"],
    }
    for experiment_id, conditions in swap_conditions.items():
        definition = candidates.get(experiment_id, {})
        expected_duration = 120 if experiment_id == "e-swap-independent-sessions" else 300
        if {
            key: definition.get(key)
            for key in (
                "conditions",
                "repetitions",
                "events_per_run",
                "event_classes",
                "warmup_secs",
                "measurement_secs",
                "rate_msg_s",
                "payload_bytes",
                "ordering",
            )
        } != {
            "conditions": conditions,
            "repetitions": 5,
            "events_per_run": 50,
            "event_classes": ["first-use-aot", "cached"],
            "warmup_secs": 30,
            "measurement_secs": expected_duration,
            "rate_msg_s": 1000,
            "payload_bytes": 128,
            "ordering": {"method": "seeded run order", "default_seed": 1729},
        }:
            errors.append(f"candidate {experiment_id} run/event grid differs from the frozen contract")
    host = candidates.get("e-host-thermal-storage", {})
    if host.get("conditions") != ["idle", "sut-core-load-1", "sut-core-load-2", "sut-core-load-3", "cpu-memory", "usb-write", "usb-read", "cpu-memory-usb"] or host.get("repetitions") != 1:
        errors.append("thermal/storage condition grid differs from the frozen contract")
    profile = candidates.get("e-compare-ekuiper-profile", {})
    if {
        "rates_msg_s": profile.get("rates_msg_s"),
        "profiler_states": profile.get("profiler_states"),
        "repetitions": profile.get("repetitions"),
        "warmup_secs": profile.get("warmup_secs"),
        "measurement_secs": profile.get("measurement_secs"),
        "loadgen_profile": profile.get("loadgen_profile"),
        "config": profile.get("config"),
        "profiler": profile.get("profiler"),
        "ordering": profile.get("ordering"),
        "no_pool_with": profile.get("no_pool_with"),
    } != {
        "rates_msg_s": [1000, 4000, 8000],
        "profiler_states": ["profiled", "unprofiled-control"],
        "repetitions": 5,
        "warmup_secs": 30,
        "measurement_secs": 60,
        "loadgen_profile": "eval/loadgen/telemetry-120b.toml",
        "config": "eval/configs/canonical/e-perf-1-ekuiper.toml",
        "profiler": {
            "kind": "external-procfs-process-sampler",
            "interval_secs": 1,
            "maximum_rows_per_run": 62,
            "gc_runtime_metrics": "unavailable-unless-validated-runtime-interface",
            "graceful_unavailable": True,
        },
        "ordering": {
            "method": "seeded paired condition shuffle per repetition",
            "default_seed": 1729,
        },
        "no_pool_with": ["e-perf-1", "e-perf-10", "prior diagnostic rehearsals"],
    }:
        errors.append("eKuiper profile diagnostic contract differs from the frozen contract")

    interval = enhanced.get("instrumentation", {}).get("bounded_interval_metrics", {})
    if interval.get("applies_to") != "all canonical and candidate timed runs with latency or throughput outputs" or interval.get("bucket_width_ms") != 1000 or interval.get("required_output") != "interval-metrics.json":
        errors.append("bounded interval contract differs from the frozen contract")
    forbidden = set(interval.get("forbidden_fields", []))
    if forbidden != {"message_id", "sequence_id", "per_message_timestamp"}:
        errors.append("bounded interval forbidden fields differ from the frozen contract")
    swap = enhanced.get("instrumentation", {}).get("multi_resolution_swap_buckets", {})
    if {key: swap.get(key) for key in ("applies_to", "canonical_bucket_width_ms", "event_bucket_width_ms", "event_window_start_ms", "event_window_end_ms", "event_bucket_count", "alignment")} != {
        "applies_to": ["e-swap-3", "e-swap-4"], "canonical_bucket_width_ms": 100, "event_bucket_width_ms": 10, "event_window_start_ms": -2000,
        "event_window_end_ms": 2000, "event_bucket_count": 400, "alignment": "actual-t0",
    }:
        errors.append("multi-resolution swap bucket contract differs from the frozen contract")

    frozen_primary = enhanced.get("primary_invariants", {})
    if frozen_primary.get("delivery_good") != {
        "loss_aggregation": "sum(total_undelivered) / sum(intended)",
        "max_loss_percent": 1.0,
        "achieved_aggregation": "mean(run achieved_rate / intended_rate)",
        "min_achieved_ratio": 0.99,
        "duplicates_allowed": 0,
    }:
        errors.append("enhanced delivery-good declaration differs from the frozen primary")
    if frozen_primary.get("mqtt_support_path_censoring") != "MQTT-loopback delivery-bad rates censor SUT-only capacity at that rate and above":
        errors.append("enhanced MQTT censoring declaration differs from the frozen primary")
    if frozen_primary.get("canonical_aliases") != {
        "e-perf-2": "e-perf-1", "e-perf-8": "e-perf-6",
        "e-swap-2": "e-swap-1", "e-swap-6": "e-swap-1",
    }:
        errors.append("enhanced alias declaration differs from the frozen primary")
    if frozen_primary.get("e_swap_4") != {
        "primary_bucket_count": 1200, "primary_window_secs": [0, 120],
        "drain_bucket_count": 100, "drain_window_secs": [120, 130],
        "after_drain_events_allowed": 0, "right_censoring_allowed": False,
    }:
        errors.append("enhanced E-Swap-4 declaration differs from the frozen primary")
    if frozen_primary.get("e_perf_10_trace_free") is not True:
        errors.append("enhanced E-Perf-10 declaration must remain trace-free")

    primary = matrix.get("experiments", {})
    envelope = primary.get("e-perf-10", {}).get("capacity_envelope", {})
    if {key: envelope.get(key) for key in ("max_loss_percent", "min_achieved_ratio", "loss_aggregation", "achieved_aggregation", "support_path_censoring")} != {
        "max_loss_percent": 1.0,
        "min_achieved_ratio": 0.99,
        "loss_aggregation": "sum(total_undelivered) / sum(intended)",
        "achieved_aggregation": "mean(run achieved_rate / intended_rate)",
        "support_path_censoring": "mqtt-loopback",
    }:
        errors.append("e-perf-10 delivery-good estimator differs from the frozen primary")
    if {"published.csv", "received.csv"} & set(primary.get("e-perf-10", {}).get("required_outputs", [])):
        errors.append("e-perf-10 final outputs must remain trace-free")
    alias_ids = ("e-perf-1", "e-perf-2", "e-perf-6", "e-perf-8", "e-swap-1", "e-swap-2", "e-swap-6")
    aliases = {key: primary.get(key, {}).get("shares_measurements_with") for key in alias_ids}
    if aliases != {
        "e-perf-1": ["e-perf-2"], "e-perf-2": ["e-perf-1"],
        "e-perf-6": ["e-perf-8"], "e-perf-8": ["e-perf-6"],
        "e-swap-1": ["e-swap-2", "e-swap-6"],
        "e-swap-2": ["e-swap-1", "e-swap-6"], "e-swap-6": ["e-swap-1", "e-swap-2"],
    }:
        errors.append("canonical alias identity differs from the frozen primary")
    tail = primary.get("e-swap-4", {}).get("sink_tail_policy", {})
    if {key: tail.get(key) for key in ("primary_start_secs", "primary_end_secs", "primary_bucket_count", "drain_start_secs", "drain_end_secs", "drain_bucket_count", "after_drain_events_allowed", "require_full_sequence_reconciliation")} != {
        "primary_start_secs": 0, "primary_end_secs": 120, "primary_bucket_count": 1200,
        "drain_start_secs": 120, "drain_end_secs": 130, "drain_bucket_count": 100,
        "after_drain_events_allowed": 0, "require_full_sequence_reconciliation": True,
    }:
        errors.append("e-swap-4 primary/drain boundary differs from the frozen primary")
    if primary.get("e-perf-5", {}).get("claim_status") != "pending-until-x86":
        errors.append("e-perf-5 must remain PENDING")

    deferred = enhanced.get("deferred_claims", {})
    if deferred.get("external_total_input_power") != "deferred" or deferred.get("pmic_measurement_boundary") != "internal-rail-proxy":
        errors.append("PMIC measurement boundary must remain internal-rail-proxy")
    storage = enhanced.get("storage", {})
    volume_label = storage.get("volume_label")
    if not isinstance(volume_label, str) or len(volume_label.encode("utf-16-le")) // 2 > 11:
        errors.append("exFAT volume label exceeds 11 UTF-16 code units")
    if {key: storage.get(key) for key in ("filesystem", "volume_label", "physical_raw_copies", "manifest_paths", "directories", "analysis_write_roots")} != {
        "filesystem": "exFAT", "volume_label": "WAF_RESULTS", "physical_raw_copies": 1,
        "manifest_paths": "volume-root-relative", "directories": ["raw", "manifests", "derived", "reports"],
        "analysis_write_roots": ["derived", "reports"],
    } or "append-only" not in str(storage.get("raw_policy", "")) or "never duplicate" not in str(storage.get("raw_policy", "")):
        errors.append("single-copy storage policy differs from the frozen contract")
    if storage.get("mount_paths") != {"pi_jetson": "/mnt/wafer-results", "macos": "/Volumes/WAF_RESULTS"}:
        errors.append("storage mount paths differ from the frozen contract")

    if decision.get("source_of_truth") != "eval/canonical-matrix.json#enhanced_candidate":
        errors.append("architecture decision source of truth differs from the matrix")
    if set(decision.get("candidate_experiment_ids", [])) != CANDIDATE_IDS:
        errors.append("architecture decision candidate IDs differ from the matrix")
    admission = decision.get("evidence_admission", {})
    if admission.get("candidate_thesis_evidence") is not False or admission.get("candidate_n30_admitted") is not False or admission.get("campaign_started") is not False:
        errors.append("architecture decision admits candidate evidence before selection")
    for phrase in CONTRACT_PHRASES:
        if phrase not in contract:
            errors.append(f"result contract missing required phrase: {phrase}")
    return errors


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--matrix", type=Path, required=True)
    parser.add_argument("--decision", type=Path, required=True)
    parser.add_argument("--contract", type=Path, required=True)
    args = parser.parse_args()
    try:
        matrix = load(args.matrix)
        decision = load(args.decision)
        contract = args.contract.read_text()
    except (ValueError, OSError) as error:
        print(f"ERROR: {error}", file=sys.stderr)
        return 1
    errors = validate(matrix, decision, contract)
    if errors:
        for error in errors:
            print(f"ERROR: {error}", file=sys.stderr)
        return 1
    print(f"enhanced architecture: PASS ({len(CANDIDATE_IDS)} candidate experiments)")
    print("capabilities: " + ", ".join(sorted(CAPABILITIES)))
    print("experiments: " + ", ".join(sorted(CANDIDATE_IDS)))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
