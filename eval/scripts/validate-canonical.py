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
from host_facts import cpu_governors, cpu_policy_facts, platform_facts  # noqa: E402
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
PAYLOAD_SIZES = ("120b", "1kb", "10kb", "100kb")
NATIVE_PASS_THROUGH = {"kind": "native", "function": "passthrough"}
SWAP_SESSION_EXPERIMENTS = {"e-swap-1", "e-swap-2", "e-swap-5", "e-swap-6"}
SWAP_SESSION_RUNS = 10
SWAP_SESSION_EVENTS = 50
VERDICT_THRESHOLD_FIELDS = {
    "criterion",
    "experiments",
    "value",
    "unit",
    "direction",
    "statistic",
    "rule",
    "role",
    "origin",
    "pilot_data_visible",
}
VERDICT_RULES = {"one-sided-bound", "exact-count", "every-run", "per-rate-cell", "tested-rate-bracket"}
VERDICT_ROLES = {"criterion", "reference", "gate"}
VERDICT_DIRECTIONS = {"<", "<=", ">", ">="}


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


def collect_host_facts(root: Path, host: str = "rpi5") -> dict:
    sha, dirty, tags = source_facts(root)
    governors = cpu_governors()
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
    if campaign.get("thesis_evidence") is not True:
        errors.append("final_campaign thesis_evidence must be true")
    grid = dict(campaign.get("capacity_grid", {}))
    if campaign.get("diagnostic_batches_excluded") is not True:
        errors.append("final_campaign must exclude diagnostic batches")
    sweep = experiments.get("e-perf-10", {})
    if sweep.get("rate_points_msg_s") != grid.get("common_rate_points_msg_s"):
        errors.append("e-perf-10 rate grid differs from final_campaign capacity grid")
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

    errors.extend(validate_payload_arms(experiments.get("e-perf-4", {}), campaign))
    errors.extend(validate_verdict_rules(matrix))
    errors.extend(validate_replication_concordance(matrix))

    records = 0
    for experiment_id, definition in experiments.items():
        multiplier = len(definition.get("conditions", []))
        if experiment_id == "e-perf-10":
            multiplier *= len(definition.get("rate_points_msg_s", []))
        records += int(definition.get("repetitions", 0)) * multiplier
    if campaign.get("expected_schedule_records") != records:
        errors.append("final_campaign expected_schedule_records differs from calculated count")

    return errors


def validate_verdict_rules(matrix: dict) -> list[str]:
    rules = matrix.get("verdict_rules")
    if not isinstance(rules, dict):
        return ["verdict_rules must be an object holding the declared threshold table"]
    errors = []
    if rules.get("schema_version") != 1:
        errors.append("verdict_rules schema_version must be 1")
    for field in ("resampling", "pilot_data"):
        if not isinstance(rules.get(field), str) or not rules[field]:
            errors.append(f"verdict_rules {field} must be a non-empty string")
    thresholds = rules.get("thresholds")
    if not isinstance(thresholds, list) or not thresholds:
        return [*errors, "verdict_rules thresholds must be a non-empty list"]
    declared = {}
    for index, row in enumerate(thresholds):
        name = row.get("criterion") if isinstance(row, dict) else None
        label = f"verdict threshold {name or index}"
        if not isinstance(row, dict) or set(row) != VERDICT_THRESHOLD_FIELDS:
            errors.append(f"{label} must have exactly the fields {sorted(VERDICT_THRESHOLD_FIELDS)}")
            continue
        experiments = row["experiments"]
        if (
            not isinstance(name, str)
            or not name
            or not isinstance(experiments, list)
            or not experiments
            or not set(experiments) <= EXPECTED_EXPERIMENTS
            or type(row["value"]) not in (int, float)
            or row["direction"] not in VERDICT_DIRECTIONS
            or row["rule"] not in VERDICT_RULES
            or row["role"] not in VERDICT_ROLES
            or row["pilot_data_visible"] not in (True, False, "unknown")
            or any(not isinstance(row[field], str) or not row[field] for field in ("unit", "statistic", "origin"))
        ):
            errors.append(f"{label} is malformed")
            continue
        if name in declared:
            errors.append(f"{label} is declared twice")
            continue
        declared[name] = (
            experiments,
            row["value"],
            row["unit"],
            row["direction"],
            row["rule"],
            row["role"],
        )
    envelope = matrix.get("experiments", {}).get("e-perf-10", {}).get("capacity_envelope", {})
    restated = {
        "e-perf-10-competitive-ratio": envelope.get("competitive_ratio_threshold"),
        "e-perf-10-cell-loss": (
            envelope["max_loss_percent"] / 100
            if type(envelope.get("max_loss_percent")) in (int, float)
            else None
        ),
        "e-perf-10-cell-achieved-ratio": envelope.get("min_achieved_ratio"),
        "e-swap-3-dip": matrix.get("experiments", {})
        .get("e-swap-3", {})
        .get("event_alignment", {})
        .get("max_hot_swap_dip_percent"),
    }
    for name, value in restated.items():
        if name in declared and declared[name][1] != value:
            errors.append(f"verdict threshold {name} disagrees with the value the experiment declares")
    return errors


def validate_replication_concordance(matrix: dict) -> list[str]:
    rule = matrix.get("replication_concordance")
    if not isinstance(rule, dict):
        return ["replication_concordance must be an object declaring how replication hosts agree"]
    errors = []
    hosts = matrix.get("hosts", {})
    if rule.get("canonical_host") != matrix.get("canonical_host"):
        errors.append("replication_concordance canonical_host must be the matrix canonical_host")
    if rule.get("replication_hosts") != [
        name for name, host in hosts.items() if isinstance(host, dict) and host.get("role") == "replication"
    ]:
        errors.append("replication_concordance replication_hosts must list every replication host")
    return errors


def validate_payload_arms(payload: dict, campaign: dict) -> list[str]:
    errors = []
    if payload.get("conditions") != [*PAYLOAD_SIZES, *(f"native-{size}" for size in PAYLOAD_SIZES)]:
        errors.append("e-perf-4 must pair every WAFER payload size with a native arm")
    if not {"service.hdr", "service-percentiles.json"} <= set(payload.get("required_outputs", [])):
        errors.append("e-perf-4 required outputs lack the service-time histogram and summary")
    wafer_configs = {
        entry.get("condition"): entry.get("config")
        for entry in campaign.get("wafer_config_catalog", [])
        if entry.get("experiment") == "e-perf-4"
    }
    for size in PAYLOAD_SIZES:
        try:
            wafer = tomllib.loads((ROOT / wafer_configs[size]).read_text())
            native = tomllib.loads(
                (ROOT / f"eval/configs/canonical/e-perf-4-native-{size}.toml").read_text()
            )
            native_plugin = native["nodes"]["transform"]["plugin"]
            wafer_plugin = wafer["nodes"]["transform"]["plugin"]
        except (KeyError, OSError, TypeError, tomllib.TOMLDecodeError):
            errors.append(f"e-perf-4 {size} WAFER or native config is missing or malformed")
            continue
        if (
            native_plugin != NATIVE_PASS_THROUGH
            or not str(wafer_plugin).endswith("wafer_pass_through.wasm")
            or "engine" in native
            or _without_transform_plugin(native) != _without_transform_plugin(wafer)
        ):
            errors.append(
                f"e-perf-4 {size} native arm differs from the WAFER arm beyond the transform"
            )
    return errors


def _without_transform_plugin(config: dict) -> dict:
    nodes = {
        name: {key: value for key, value in node.items() if name != "transform" or key != "plugin"}
        for name, node in config["nodes"].items()
    }
    return {
        **{key: value for key, value in config.items() if key not in {"pipeline", "engine"}},
        "nodes": nodes,
    }


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
