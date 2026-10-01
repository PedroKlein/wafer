#!/usr/bin/env python3

from __future__ import annotations

import argparse
import json
import re
import socket
import subprocess
import sys
import tomllib
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent / "lib"))
from host_facts import cpu_policy_facts, platform_facts  # noqa: E402
from host_profiles import HostProfile, host_profile, host_profiles  # noqa: E402
from pi_telemetry import host_snapshot  # noqa: E402

ROOT = Path(__file__).resolve().parents[2]

EXPECTED_EXPERIMENTS = {
    "e-val-1",
    *(f"e-perf-{i}" for i in range(1, 11)),
    *(f"e-iso-{i}" for i in range(1, 9)),
    *(f"e-swap-{i}" for i in range(1, 7)),
    "e-backpressure",
    "e-density-1",
}
REQUIRED_FIELDS = {
    "research_question",
    "purpose",
    "sample_unit",
    "repetitions",
    "warmup_secs",
    "measurement_secs",
    "conditions",
    "required_outputs",
    "analysis",
    "thesis_evidence",
    "evidence_class",
}
FINAL_CAPACITY_RATES = [1_000, 4_000, 8_000, 15_000, 16_000]
FINAL_SCHEDULE_RECORDS = 2_201
FINAL_MEASURED_LEAVES = 1_971
SWAP_SESSION_EXPERIMENTS = {"e-swap-1", "e-swap-2", "e-swap-5", "e-swap-6"}
SWAP_SESSION_RUNS = 10
SWAP_SESSION_EVENTS = 50


def load_object(path: Path) -> dict:
    try:
        value = json.loads(path.read_text())
    except OSError as error:
        raise ValueError(f"cannot read {path}: {error}") from error
    except json.JSONDecodeError as error:
        raise ValueError(f"invalid JSON in {path}: {error}") from error
    if not isinstance(value, dict):
        raise ValueError(f"{path} must contain a JSON object")
    return value


def command_output(command: list[str], default: str = "unknown") -> str:
    try:
        return subprocess.check_output(
            command, text=True, stderr=subprocess.DEVNULL
        ).strip() or default
    except (OSError, subprocess.CalledProcessError):
        return default


def source_facts(root: Path) -> tuple[str, bool, list[str]]:
    sha = command_output(["git", "-C", str(root), "rev-parse", "HEAD"])
    if sha != "unknown":
        try:
            status_result = subprocess.run(
                ["git", "-C", str(root), "status", "--porcelain"],
                text=True,
                capture_output=True,
                check=True,
            )
            dirty = bool(status_result.stdout.strip())
        except (OSError, subprocess.CalledProcessError):
            dirty = True
        tags = [
            line
            for line in command_output(
                ["git", "-C", str(root), "tag", "--points-at", sha], default=""
            ).splitlines()
            if line
        ]
        return sha, dirty, tags

    try:
        state = load_object(root / "SOURCE_STATE.json")
    except ValueError:
        return "unknown", True, []
    tags = state.get("git_tags", [])
    if not isinstance(tags, list):
        tags = []
    return str(state.get("git_sha", "unknown")), state.get("git_dirty") is True, tags


def read_text(path: Path, default: str = "unknown") -> str:
    try:
        return path.read_text().replace("\x00", "").strip() or default
    except OSError:
        return default


def collect_host_facts(root: Path, host: str = "rpi5") -> dict:
    sha, dirty, tags = source_facts(root)
    governors = sorted(
        {
            read_text(path)
            for path in Path("/sys/devices/system/cpu").glob(
                "cpu[0-9]*/cpufreq/scaling_governor"
            )
        }
    )
    broker_ready = False
    try:
        with socket.create_connection(("127.0.0.1", 1883), timeout=1):
            broker_ready = True
    except OSError:
        pass

    _temperature, throttled = host_snapshot()

    ekuiper_ready = command_output(
        ["systemctl", "is-active", "kuiper.service"], default="inactive"
    ) == "active"
    ekuiper_version = command_output(
        ["dpkg-query", "-W", "-f=${Version}", "kuiper"]
    )
    version_match = re.search(r"\b(\d+\.\d+\.\d+)\b", ekuiper_version)

    platform = platform_facts()
    return {
        **platform,
        "host_tag": host,
        "arch": command_output(["uname", "-m"]),
        "hardware_model": platform["hardware_model"] or "unknown",
        "git_sha": sha,
        "git_dirty": dirty,
        "git_tags": tags,
        "cpu_governors": governors,
        **cpu_policy_facts(),
        "throttled": throttled,
        "broker_ready": broker_ready,
        "ekuiper_ready": ekuiper_ready,
        "ekuiper_version": version_match.group(1) if version_match else "unknown",
    }


def validate_hosts(matrix: dict) -> list[str]:
    if matrix.get("schema_version") != 2:
        return ["schema_version must be 2"]
    if matrix.get("canonical_host") != "rpi5":
        return ["canonical_host must be rpi5"]
    try:
        profiles = host_profiles(matrix)
    except (KeyError, TypeError, ValueError, AttributeError) as error:
        return [f"hosts map is malformed: {error}"]
    errors = []
    if set(profiles) != {"rpi5", "jetson", "x86"}:
        errors.append(f"hosts must be rpi5, jetson and x86, got {sorted(profiles)}")
    for tag, profile in profiles.items():
        expected_role = "canonical" if tag == matrix["canonical_host"] else "replication"
        if profile.role != expected_role:
            errors.append(f"host {tag} role must be {expected_role}")
        if profile.throttled != "0x0" or profile.cpu_governors != ("performance",):
            errors.append(f"host {tag} must require no throttling and the performance governor")
        if _cpu_set(profile.sut_cpus) & _cpu_set(profile.support_cpus):
            errors.append(f"host {tag} SUT and support CPUs overlap")
        if _cpu_set(profile.housekeeping_cpus) & _cpu_set(profile.sut_cpus):
            errors.append(f"host {tag} housekeeping and SUT CPUs overlap")
        if not _cpu_set(profile.support_cpus) <= _cpu_set(profile.housekeeping_cpus):
            errors.append(f"host {tag} support CPUs must be housekeeping CPUs")
    return errors


def _cpu_set(text: str) -> set[int]:
    cpus: set[int] = set()
    for part in text.split(","):
        low, _, high = part.partition("-")
        cpus.update(range(int(low), int(high or low) + 1))
    return cpus


def validate_matrix(matrix: dict) -> list[str]:
    errors: list[str] = []
    experiments = matrix.get("experiments")
    if not isinstance(experiments, dict):
        return ["experiments must be an object"]

    ids = set(experiments)
    missing = sorted(EXPECTED_EXPERIMENTS - ids)
    extra = sorted(ids - EXPECTED_EXPERIMENTS)
    if missing:
        errors.append(f"missing experiments: {', '.join(missing)}")
    if extra:
        errors.append(f"unknown experiments: {', '.join(extra)}")

    errors.extend(validate_hosts(matrix))

    for experiment_id, experiment in sorted(experiments.items()):
        if not isinstance(experiment, dict):
            errors.append(f"{experiment_id} must be an object")
            continue
        absent = sorted(REQUIRED_FIELDS - set(experiment))
        if absent:
            errors.append(f"{experiment_id} missing fields: {', '.join(absent)}")
            continue

        sample_unit = experiment["sample_unit"]
        repetitions = experiment["repetitions"]
        if experiment.get("thesis_evidence") is not True:
            errors.append(f"{experiment_id} thesis_evidence must be true for the final campaign")
        if experiment.get("evidence_class") != "final":
            errors.append(f"{experiment_id} evidence_class must be final")
        warmup_secs = experiment["warmup_secs"]
        measurement_secs = experiment["measurement_secs"]
        conditions = experiment["conditions"]
        outputs = experiment["required_outputs"]

        if sample_unit not in {"run", "static"}:
            errors.append(f"{experiment_id} has invalid sample_unit {sample_unit!r}")
        if not isinstance(repetitions, int) or repetitions < 1:
            errors.append(f"{experiment_id} repetitions must be a positive integer")
        elif experiment_id in SWAP_SESSION_EXPERIMENTS:
            if repetitions != SWAP_SESSION_RUNS:
                errors.append(f"{experiment_id} repetitions must be exactly {SWAP_SESSION_RUNS}")
            if experiment.get("events_per_run") != SWAP_SESSION_EVENTS:
                errors.append(f"{experiment_id} events_per_run must be exactly {SWAP_SESSION_EVENTS}")
        elif sample_unit == "run" and repetitions < 30:
            errors.append(f"{experiment_id} repetitions must be >= 30")
        if sample_unit == "static" and repetitions != 1:
            errors.append(f"{experiment_id} static measurement must have one repetition")
        if experiment_id in {"e-swap-1", "e-swap-2", "e-swap-4", "e-swap-5", "e-swap-6"}:
            expected_nested = (
                "rollback event within run"
                if experiment_id == "e-swap-5"
                else "one swap within run"
                if experiment_id == "e-swap-4"
                else "swap event within run"
            )
            if experiment.get("independent_unit") != "complete process run":
                errors.append(f"{experiment_id} independent_unit must be complete process run")
            if experiment.get("nested_unit") != expected_nested:
                errors.append(f"{experiment_id} nested_unit is inconsistent")

        if not isinstance(warmup_secs, int) or warmup_secs < 0:
            errors.append(f"{experiment_id} warmup_secs must be a non-negative integer")
        elif warmup_secs < 30 and not experiment.get("warmup_exception"):
            errors.append(
                f"{experiment_id} warmup_secs must be >= 30 or have warmup_exception"
            )
        if not isinstance(measurement_secs, int) or measurement_secs < 0:
            errors.append(
                f"{experiment_id} measurement_secs must be a non-negative integer"
            )
        elif sample_unit != "static" and measurement_secs == 0:
            errors.append(f"{experiment_id} measurement_secs must be positive")
        if not isinstance(conditions, list) or not conditions or not all(
            isinstance(value, str) and value for value in conditions
        ):
            errors.append(f"{experiment_id} conditions must be non-empty strings")
        if not isinstance(outputs, list) or not outputs or not all(
            isinstance(value, str) and value and Path(value).name == value
            for value in outputs
        ):
            errors.append(f"{experiment_id} required_outputs must be file names")
        analysis = experiment["analysis"]
        if not isinstance(analysis, str) or not analysis.endswith(".ipynb"):
            errors.append(f"{experiment_id} analysis must name a notebook")

    campaign = matrix.get("final_campaign")
    if not isinstance(campaign, dict):
        errors.append("final_campaign must be an object")
        return errors
    if campaign.get("status") != "frozen-before-execution":
        errors.append("final_campaign status must be frozen-before-execution")
    if campaign.get("seed") != 1729:
        errors.append("final_campaign seed must be 1729")
    if campaign.get("thesis_evidence") is not True:
        errors.append("final_campaign thesis_evidence must be true")
    grid = campaign.get("capacity_grid", {})
    if grid != {
        "source_batch_id": "capacity-scout-v3-20260904T045000Z",
        "source_summary_sha256": "04531979da50f882eee2e0d04ab6f25d4002af21519a4c8b5ada6c88c13452b5",
        "candidate_sha256": "5f2231ef541c36c4fef3655ed25239ca028fb7ed7cd1387a3dd6644818cbfc3f",
        "common_rate_points_msg_s": FINAL_CAPACITY_RATES,
    }:
        errors.append("final_campaign capacity grid differs from the frozen scout-derived grid")
    metering = campaign.get("canonical_metering", {})
    if metering != {
        "policy": "explicit-fuel-and-epoch",
        "fuel": {"transform": 10_000_000, "filter": 500_000, "router": 500_000},
        "epoch_deadline": 100,
        "epoch_tick_ms": 10,
    }:
        errors.append("final_campaign canonical metering policy is invalid")
    if campaign.get("ekuiper_operator_concurrency") != 1:
        errors.append("final_campaign eKuiper operator concurrency must be 1")
    if campaign.get("diagnostic_batches_excluded") is not True:
        errors.append("final_campaign must exclude diagnostic batches")
    if campaign.get("attempt_policy") != {
        "infrastructure_retries": 1,
        "gate_experiments": ["e-val-1"],
    }:
        errors.append(
            "final_campaign attempt policy must allow one in-place infrastructure retry "
            "and none for the E-Val-1 gate"
        )

    expected_metering_exceptions = {
        "e-perf-5": {
            "wafer": {
                "policy": "explicit-fuel-and-epoch-for-present-node-categories",
                "fuel": {"transform": 10_000_000, "filter": None, "router": None},
                "epoch_deadline": 100,
                "epoch_tick_ms": 10,
            }
        },
        "e-iso-4": {
            "infinite-loop": {
                "type": "epoch-containment-stimulus",
                "rationale": "Fuel must not preempt the intended epoch-containment mechanism.",
                "fuel": None,
                "epoch_deadline": 1,
                "epoch_tick_ms": 10,
            }
        },
        "e-iso-5": {
            "memory-exhaust": {
                "type": "memory-limit-stimulus",
                "rationale": "Fuel must not preempt the intended memory-limit mechanism.",
                "fuel": None,
                "epoch_deadline": 100,
                "epoch_tick_ms": 10,
            }
        },
        "e-iso-7": {
            "epoch-loop-attack": {
                "type": "epoch-containment-stimulus",
                "rationale": "Fuel must not preempt the intended epoch-containment mechanism.",
                "fuel": None,
                "epoch_deadline": 100,
                "epoch_tick_ms": 10,
            }
        },
    }
    for experiment_id, expected in expected_metering_exceptions.items():
        if experiments.get(experiment_id, {}).get("metering_exceptions") != expected:
            errors.append(f"{experiment_id} metering exceptions differ from the frozen policy")
    undeclared = sorted(
        experiment_id
        for experiment_id, definition in experiments.items()
        if "metering_exceptions" in definition and experiment_id not in expected_metering_exceptions
    )
    if undeclared:
        errors.append(f"undeclared metering exceptions: {', '.join(undeclared)}")

    metering_modes = experiments.get("e-perf-7", {}).get("metering_modes")
    if metering_modes != {
        "neither": {"fuel": None, "epoch_deadline": None},
        "fuel-only": {"fuel": 10_000_000, "epoch_deadline": None},
        "epoch-only": {"fuel": None, "epoch_deadline": 100},
        "both": {"fuel": 10_000_000, "epoch_deadline": 100},
    }:
        errors.append("e-perf-7 metering modes differ from the Option-value truth table")

    sweep = experiments.get("e-perf-10", {})
    if sweep.get("repetitions") != 30:
        errors.append("e-perf-10 repetitions must be exactly 30")
    if sweep.get("sample_unit") != "run":
        errors.append("e-perf-10 sample_unit must be run")
    if sweep.get("rate_points_msg_s") != FINAL_CAPACITY_RATES:
        errors.append("e-perf-10 rate grid differs from the frozen scout-derived grid")
    if sweep.get("rate_points_msg_s") != grid.get("common_rate_points_msg_s"):
        errors.append("e-perf-10 rate grid differs from final_campaign capacity grid")
    capacity_outputs = {
        "latency.hdr", "throughput.csv", "sequence.csv", "subscriber-metadata.json",
        "publisher-summary.json", "capacity-run.json", "resource-usage.csv",
        "process-audit.json",
    }
    if set(sweep.get("required_outputs", [])) != capacity_outputs:
        errors.append("e-perf-10 required outputs differ from the trace-free final contract")

    swap3 = experiments.get("e-swap-3", {})
    if swap3.get("sample_unit") != "run" or swap3.get("repetitions") != 30:
        errors.append("e-swap-3 must use 30 run-level repetitions")
    if swap3.get("conditions") != ["wafer-hotswap", "wafer-restart", "ekuiper-restart"]:
        errors.append("e-swap-3 conditions differ from the frozen strategies")
    swap3_outputs = {
        "latency.hdr",
        "throughput.csv",
        "sequence.csv",
        "publisher-summary.json",
        "subscriber-metadata.json",
        "throughput-buckets.json",
        "throughput-buckets-10ms.json",
        "disruption-timeline.json",
        "disruption-analysis.json",
    }
    if set(swap3.get("required_outputs", [])) != swap3_outputs:
        errors.append("e-swap-3 required outputs differ from the final disruption contract")
    alignment = swap3.get("event_alignment", {})
    if alignment != {
        "event_at_secs": 60,
        "bucket_width_ms": 100,
        "series_start_secs": -10,
        "series_end_secs": 10,
        "baseline_window_secs": [-10, -2],
        "event_window_secs": [-2, 2],
        "recovery_window_secs": [2, 10],
        "recovery_fraction": 0.95,
        "recovery_consecutive_buckets": 5,
        "max_hot_swap_dip_percent": 5.0,
    }:
        errors.append("e-swap-3 event alignment differs from the frozen estimator")

    swap4 = experiments.get("e-swap-4", {})
    if swap4.get("repetitions") != 30:
        errors.append("e-swap-4 repetitions must be exactly 30")
    if swap4.get("sample_unit") != "run":
        errors.append("e-swap-4 sample_unit must be run")
    if "events_per_run" in swap4:
        errors.append("e-swap-4 must not declare correlated events_per_run")
    if swap4.get("burst_profile") != {
        "before_rate_msg_s": 1_000,
        "burst_rate_msg_s": 2_000,
        "after_rate_msg_s": 1_000,
        "burst_start_secs": 55,
        "swap_secs": 60,
        "burst_end_secs": 65,
        "swaps_per_run": 1,
    }:
        errors.append("e-swap-4 burst profile differs from the frozen one-swap design")
    if swap4.get("sink_tail_policy") != {
        "alignment_clock": "unix-epoch-source-sink-alignment",
        "primary_start_secs": 0,
        "primary_end_secs": 120,
        "primary_bucket_count": 1_200,
        "drain_start_secs": 120,
        "drain_end_secs": 130,
        "drain_bucket_count": 100,
        "bucket_width_ms": 100,
        "after_drain_events_allowed": 0,
        "source_completion_deadline_secs": 130,
        "require_full_sequence_reconciliation": True,
    }:
        errors.append("e-swap-4 sink tail policy differs from the frozen v3 design")
    swap4_outputs = {
        "latency.hdr",
        "throughput.csv",
        "sequence.csv",
        "throughput-buckets.json",
        "throughput-buckets-10ms.json",
        "burst-source-timing.json",
        "burst-source-summary.json",
        "burst-timeline.json",
        "swap-actual-t0.json",
        "swap_timeline.json",
        "hotswap-analysis.json",
        "swap_requests.json",
    }
    if set(swap4.get("required_outputs", [])) != swap4_outputs:
        errors.append("e-swap-4 required outputs differ from the final one-event contract")

    backpressure = experiments.get("e-backpressure", {})
    backpressure_configs = {
        "slow": "eval/configs/e-backpressure/pipeline-saturated.toml",
        "drop": "eval/configs/e-backpressure/pipeline-drop.toml",
        "dead-letter": "eval/configs/e-backpressure/pipeline-dead-letter.toml",
    }
    if backpressure.get("conditions") != list(backpressure_configs):
        errors.append("e-backpressure conditions differ from the policy contract")
    if backpressure.get("policy_configs") != backpressure_configs:
        errors.append("e-backpressure policy configs differ from the frozen paths")
    if backpressure.get("measured_queue") != "slow":
        errors.append("e-backpressure measured queue must be slow")
    for policy, relative_path in backpressure_configs.items():
        try:
            config = tomllib.loads((ROOT / relative_path).read_text())
            measured_edges = [
                edge for edge in config["edges"] if edge["to"] == backpressure.get("measured_queue")
            ]
        except (KeyError, OSError, tomllib.TOMLDecodeError):
            errors.append(f"e-backpressure {policy} config is missing or malformed")
            continue
        if len(measured_edges) != 1 or measured_edges[0].get("overflow", "slow") != policy:
            errors.append(f"e-backpressure {policy} config has the wrong overflow policy")
        if config.get("dead_letter", {}).get("kind") != "file":
            errors.append(f"e-backpressure {policy} config has no file DLQ declaration")

    if experiments.get("e-perf-9", {}).get("cache_scope") != "linux-filesystem-page-cache":
        errors.append("e-perf-9 cache scope must be linux-filesystem-page-cache")
    if experiments.get("e-perf-5", {}).get("incomplete_until") != "matching x86 Linux batch":
        errors.append("e-perf-5 must remain incomplete until matching x86 Linux batch")
    if set(experiments.get("e-density-1", {}).get("required_outputs", [])) != {
        "binary-sizes.csv",
        "container-floor.json",
    }:
        errors.append("e-density-1 required outputs differ from the measured container-floor contract")

    records = 0
    for experiment_id, definition in experiments.items():
        multiplier = len(definition.get("conditions", []))
        if experiment_id == "e-perf-10":
            multiplier *= len(definition.get("rate_points_msg_s", []))
        records += int(definition.get("repetitions", 0)) * multiplier
    if records != FINAL_SCHEDULE_RECORDS:
        errors.append(f"final schedule record count must be {FINAL_SCHEDULE_RECORDS}, got {records}")
    if campaign.get("expected_schedule_records") != records:
        errors.append("final_campaign expected_schedule_records differs from calculated count")
    if campaign.get("expected_measured_leaves") != FINAL_MEASURED_LEAVES:
        errors.append(f"final_campaign expected_measured_leaves must be {FINAL_MEASURED_LEAVES}")

    return errors


def validate_preflight(
    facts: dict, require_ekuiper: bool, profile: HostProfile | None = None
) -> list[str]:
    errors = (profile or host_profile("rpi5")).fact_errors(facts)
    if facts.get("git_dirty") is not False:
        errors.append("dirty source is not canonical")
    if not re.fullmatch(r"[0-9a-f]{40}", str(facts.get("git_sha", ""))):
        errors.append("source commit SHA must be a full 40-character lowercase hex digest")
    if facts.get("broker_ready") is not True:
        errors.append("broker is not ready")
    if require_ekuiper:
        if facts.get("ekuiper_ready") is not True:
            errors.append("eKuiper is not ready")
        if facts.get("ekuiper_version") != "2.1.5":
            errors.append(
                f"eKuiper version must be '2.1.5', got {facts.get('ekuiper_version')!r}"
            )
    return errors


def report(errors: list[str], success: str) -> int:
    if errors:
        for error in errors:
            print(f"ERROR: {error}", file=sys.stderr)
        return 1
    print(success)
    return 0


def main() -> int:
    parser = argparse.ArgumentParser()
    subparsers = parser.add_subparsers(dest="command", required=True)

    matrix_parser = subparsers.add_parser("matrix")
    matrix_parser.add_argument("path", type=Path)

    preflight_parser = subparsers.add_parser("preflight")
    preflight_parser.add_argument("facts", type=Path)
    preflight_parser.add_argument("--require-ekuiper", action="store_true")
    preflight_parser.add_argument("--host", default="rpi5")

    host_parser = subparsers.add_parser("host")
    host_parser.add_argument("--root", type=Path, default=Path.cwd())
    host_parser.add_argument("--require-ekuiper", action="store_true")
    host_parser.add_argument("--output", type=Path)
    host_parser.add_argument("--host", default="rpi5")

    args = parser.parse_args()
    profile: HostProfile | None = None
    if args.command in {"host", "preflight"}:
        try:
            profile = host_profile(args.host)
        except (OSError, ValueError, KeyError) as error:
            return report([str(error)], "")
    if args.command == "host":
        value = collect_host_facts(args.root.resolve(), args.host)
        if args.output:
            args.output.write_text(json.dumps(value, indent=2) + "\n")
        return report(
            validate_preflight(value, args.require_ekuiper, profile),
            "canonical host preflight: PASS",
        )

    try:
        value = load_object(args.path if args.command == "matrix" else args.facts)
    except ValueError as error:
        return report([str(error)], "")

    if args.command == "matrix":
        errors = validate_matrix(value)
        count = len(value.get("experiments", {}))
        campaign = value.get("final_campaign", {})
        return report(
            errors,
            f"canonical matrix: PASS ({count} experiments, "
            f"schedule_records={campaign.get('expected_schedule_records')}, "
            f"measured_leaves={campaign.get('expected_measured_leaves')})",
        )

    return report(
        validate_preflight(value, args.require_ekuiper, profile),
        "canonical preflight: PASS",
    )


if __name__ == "__main__":
    raise SystemExit(main())
