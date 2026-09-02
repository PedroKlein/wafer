#!/usr/bin/env python3

from __future__ import annotations

import argparse
import json
import re
import socket
import subprocess
import sys
from pathlib import Path

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
}
FOCUSED_REQUIRED_FIELDS = {
    "sample_unit",
    "repetitions",
    "warmup_secs",
    "measurement_secs",
    "measurement_boundary",
    "condition_runs",
    "required_outputs",
    "analysis",
    "thesis_evidence",
}
FOCUSED_CONDITION_RUNS = {
    "e-perf-10": {
        f"{system}/rate-{rate:05d}": [1, 2, 3, 4]
        for system in ("mqtt-loopback", "native", "wafer", "ekuiper")
        for rate in (500, 1000, 2000, 4000, 8000, 16000)
    },
    "e-perf-9": {
        f"{tier}-{cache}": [1, 2, 3]
        for tier in ("small", "medium", "large")
        for cache in ("cold", "warm")
    },
    "e-backpressure": {"saturated-slow-consumer": [1, 2, 3]},
    "e-iso-4": {"infinite-loop": [1, 2, 3]},
    "e-iso-7": {
        condition: [1, 2, 3]
        for condition in ("control", "panic-attack", "epoch-loop-attack")
    },
    "e-swap-1": {"steady": [1]},
    "e-swap-2": {"steady": [1]},
    "e-swap-3": {"wafer-hotswap": [1, 2, 3]},
    "e-swap-4": {"burst-2x": [1]},
    "e-swap-5": {"process-trap-rollback": [1]},
    "e-swap-6": {"steady": [1]},
}


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


def collect_host_facts(root: Path) -> dict:
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

    throttled = command_output(["vcgencmd", "get_throttled"])
    if throttled.startswith("throttled="):
        throttled = throttled.removeprefix("throttled=")

    ekuiper_ready = command_output(
        ["systemctl", "is-active", "kuiper.service"], default="inactive"
    ) == "active"
    ekuiper_version = command_output(
        ["dpkg-query", "-W", "-f=${Version}", "kuiper"]
    )
    version_match = re.search(r"\b(\d+\.\d+\.\d+)\b", ekuiper_version)

    return {
        "host_tag": "rpi5",
        "arch": command_output(["uname", "-m"]),
        "hardware_model": read_text(Path("/proc/device-tree/model")),
        "git_sha": sha,
        "git_dirty": dirty,
        "git_tags": tags,
        "cpu_governors": governors,
        "isolated_cpus": read_text(Path("/sys/devices/system/cpu/isolated")),
        "throttled": throttled,
        "broker_ready": broker_ready,
        "ekuiper_ready": ekuiper_ready,
        "ekuiper_version": version_match.group(1) if version_match else "unknown",
    }


def validate_focused_pilot(matrix: dict, experiments: dict) -> list[str]:
    errors: list[str] = []
    focused = matrix.get("focused_pilot")
    if not isinstance(focused, dict):
        return ["focused_pilot must be an object"]
    if focused.get("schema_version") != 1:
        errors.append("focused_pilot schema_version must be 1")
    if focused.get("id") != "rpi5-focused-pilot-followups":
        errors.append("focused_pilot id must be rpi5-focused-pilot-followups")
    if focused.get("seed") != 1729:
        errors.append("focused_pilot seed must be 1729")
    if focused.get("thesis_evidence") is not False:
        errors.append("focused_pilot must set thesis_evidence=false")

    selected = focused.get("experiments")
    if not isinstance(selected, dict):
        return errors + ["focused_pilot experiments must be an object"]
    if set(selected) != set(FOCUSED_CONDITION_RUNS):
        errors.append("focused_pilot experiment selection differs from the frozen set")

    for experiment_id, expected_runs in FOCUSED_CONDITION_RUNS.items():
        definition = selected.get(experiment_id)
        if not isinstance(definition, dict):
            continue
        absent = sorted(FOCUSED_REQUIRED_FIELDS - definition.keys())
        if absent:
            errors.append(
                f"focused_pilot {experiment_id} missing fields: {', '.join(absent)}"
            )
            continue
        if definition["condition_runs"] != expected_runs:
            errors.append(f"focused_pilot {experiment_id} condition_runs differ from the frozen set")
        if definition["thesis_evidence"] is not False:
            errors.append(f"focused_pilot {experiment_id} must set thesis_evidence=false")
        if not isinstance(definition["measurement_boundary"], str) or not definition["measurement_boundary"]:
            errors.append(f"focused_pilot {experiment_id} measurement_boundary must be non-empty")
        canonical = experiments.get(experiment_id, {})
        for field in ("sample_unit", "warmup_secs", "measurement_secs", "required_outputs", "analysis"):
            if definition[field] != canonical.get(field):
                errors.append(f"focused_pilot {experiment_id} {field} differs from canonical experiment")
        repetitions = definition["repetitions"]
        if not isinstance(repetitions, int) or repetitions < 1:
            errors.append(f"focused_pilot {experiment_id} repetitions must be positive")
        elif any(len(runs) != repetitions for runs in definition["condition_runs"].values()):
            errors.append(f"focused_pilot {experiment_id} repetitions differ from condition runs")
        if definition["sample_unit"] == "event" and definition.get("events_per_run") != 50:
            errors.append(f"focused_pilot {experiment_id} events_per_run must be 50")

    decisions = focused.get("decisions")
    if not isinstance(decisions, dict):
        return errors + ["focused_pilot decisions must be an object"]
    if decisions.get("ekuiper_operator_concurrency") != {
        "value": 1,
        "comparison": "default-system",
    }:
        errors.append("focused_pilot eKuiper operator concurrency must be default-system value 1")
    memory = decisions.get("memory_retention")
    if not isinstance(memory, dict):
        errors.append("focused_pilot memory_retention decision must be an object")
    else:
        if memory.get("status") != "fixed":
            errors.append("focused_pilot memory_retention status must be fixed")
        if not re.fullmatch(r"[0-9a-f]{40}", str(memory.get("fix_commit", ""))):
            errors.append("focused_pilot memory_retention fix_commit must be a full SHA")
        if memory.get("diagnostic_runs") != 2:
            errors.append("focused_pilot memory_retention diagnostic_runs must be 2")
        if memory.get("max_observed_slope_bytes_per_message") != 0.0:
            errors.append("focused_pilot memory_retention slope must be 0 bytes/message")
        if memory.get("canonical_measurement_secs_adequate") is not True:
            errors.append("focused_pilot memory_retention must approve the canonical window")
    return errors


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

    if matrix.get("host_tag") != "rpi5":
        errors.append("host_tag must be rpi5")

    errors.extend(validate_focused_pilot(matrix, experiments))

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
        if experiment_id == "e-perf-10" and experiment.get("thesis_evidence") is not False:
            errors.append("e-perf-10 must set thesis_evidence=false")
        warmup_secs = experiment["warmup_secs"]
        measurement_secs = experiment["measurement_secs"]
        conditions = experiment["conditions"]
        outputs = experiment["required_outputs"]

        if sample_unit not in {"run", "event", "static"}:
            errors.append(f"{experiment_id} has invalid sample_unit {sample_unit!r}")
        if not isinstance(repetitions, int) or repetitions < 1:
            errors.append(f"{experiment_id} repetitions must be a positive integer")
        elif (
            sample_unit == "run"
            and repetitions < 30
            and not (
                experiment_id == "e-perf-10"
                and experiment.get("thesis_evidence") is False
            )
        ):
            errors.append(f"{experiment_id} repetitions must be >= 30")
        if sample_unit == "event":
            events = experiment.get("events_per_run")
            if not isinstance(events, int) or events < 50:
                errors.append(f"{experiment_id} events_per_run must be >= 50")
        if sample_unit == "static" and repetitions != 1:
            errors.append(f"{experiment_id} static measurement must have one repetition")

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

    return errors


def validate_preflight(facts: dict, require_ekuiper: bool) -> list[str]:
    errors: list[str] = []
    expected = {
        "host_tag": "rpi5",
        "arch": "aarch64",
        "isolated_cpus": "1-3",
        "throttled": "0x0",
    }
    labels = {
        "host_tag": "host tag",
        "arch": "architecture",
        "isolated_cpus": "isolated CPUs",
        "throttled": "throttling",
    }
    for key, value in expected.items():
        if facts.get(key) != value:
            errors.append(f"{labels[key]} must be {value!r}, got {facts.get(key)!r}")

    if "Raspberry Pi 5" not in str(facts.get("hardware_model", "")):
        errors.append("hardware model must be Raspberry Pi 5")
    if facts.get("cpu_governors") != ["performance"]:
        errors.append("CPU governor must be performance")
    if facts.get("git_dirty") is not False:
        errors.append("dirty source is not canonical")
    if not re.fullmatch(r"[0-9a-f]{40}", str(facts.get("git_sha", ""))):
        errors.append("source commit SHA must be a full 40-character lowercase hex digest")
    tags = facts.get("git_tags")
    if not isinstance(tags, list) or not tags or not all(
        isinstance(tag, str) and tag for tag in tags
    ):
        errors.append("untagged source is not canonical")
    if facts.get("broker_ready") is not True:
        errors.append("broker is not ready")
    if require_ekuiper:
        if facts.get("ekuiper_ready") is not True:
            errors.append("eKuiper is not ready")
        if facts.get("ekuiper_version") != "2.1.0":
            errors.append(
                f"eKuiper version must be '2.1.0', got {facts.get('ekuiper_version')!r}"
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

    host_parser = subparsers.add_parser("host")
    host_parser.add_argument("--root", type=Path, default=Path.cwd())
    host_parser.add_argument("--require-ekuiper", action="store_true")
    host_parser.add_argument("--output", type=Path)

    args = parser.parse_args()
    if args.command == "host":
        value = collect_host_facts(args.root.resolve())
        if args.output:
            args.output.write_text(json.dumps(value, indent=2) + "\n")
        return report(
            validate_preflight(value, args.require_ekuiper),
            "canonical host preflight: PASS",
        )

    try:
        value = load_object(args.path if args.command == "matrix" else args.facts)
    except ValueError as error:
        return report([str(error)], "")

    if args.command == "matrix":
        errors = validate_matrix(value)
        count = len(value.get("experiments", {}))
        return report(errors, f"canonical matrix: PASS ({count} experiments)")

    return report(
        validate_preflight(value, args.require_ekuiper),
        "canonical preflight: PASS",
    )


if __name__ == "__main__":
    raise SystemExit(main())
