#!/usr/bin/env python3

from __future__ import annotations

import argparse
import dataclasses
import datetime as dt
import hashlib
import json
import math
import subprocess
import sys
from collections import Counter
from pathlib import Path
from typing import Any

ROOT = Path(__file__).resolve().parents[2]
LIB = ROOT / "eval/scripts/lib"
if str(LIB) not in sys.path:
    sys.path.insert(0, str(LIB))

import canonical_runner as runner

GIB = 1024**3
MIB = 1024**2
USB_CAPACITY_BYTES = 231 * GIB
HISTORICAL_BYTES_PER_LEAF = 17_564_330
INTERVAL_BYTES_PER_ROW = 2_048
ALIAS_RECEIPT_BYTES = 4_096
INTERRUPTED_ATTEMPT_RESERVE_PERCENT = 25
HOST_USB_CORPUS_BYTES = 2 * GIB
HOST_SESSION_OTHER_BYTES = 16 * MIB
HOST_SESSION_FILES = 6
CORE_MANIFEST_FILES = 6
CORE_MANIFEST_BYTES = 16 * MIB
CANONICAL_DERIVED_FILES = 62
CANONICAL_DERIVED_BYTES = 512 * MIB
ENHANCED_DERIVED_FILES = 26
ENHANCED_DERIVED_BYTES = 256 * MIB
COMMON_LEAF_FILES = {
    "canonical-status.json",
    "config.toml",
    "invocation-receipt.json",
    "measurement-window.json",
    "metadata.json",
    "pi-telemetry.csv",
    "runtime-provenance.json",
    "stdout.log",
}
HOST_DURATION_SECONDS = 120 + 7 * 300
CANDIDATE_SUMMARY_FILES = {
    runner.CAPACITY_KNEE_EXPERIMENT: "capacity-knee-summary.json",
    runner.PAYLOAD_REFINEMENT_EXPERIMENT: "payload-refinement-summary.json",
    runner.DEPTH_EXTENSION_EXPERIMENT: "depth-extension-summary.json",
    runner.SWAP_SESSIONS_EXPERIMENT: "independent-swap-summary.json",
    runner.ROLLBACK_SESSIONS_EXPERIMENT: "rollback-session-summary.json",
    runner.EKUIPER_PROFILE_EXPERIMENT: "ekuiper-profile-summary.json",
}


def sha256(path: Path) -> str:
    return hashlib.sha256(path.read_bytes()).hexdigest()


def git_state(path: Path) -> dict[str, Any]:
    status = subprocess.run(
        ["git", "status", "--porcelain"],
        cwd=path,
        text=True,
        capture_output=True,
        check=True,
    ).stdout
    return {
        "git_sha": subprocess.run(
            ["git", "rev-parse", "HEAD"],
            cwd=path,
            text=True,
            capture_output=True,
            check=True,
        ).stdout.strip(),
        "dirty": bool(status.strip()),
        "status_sha256": hashlib.sha256(status.encode()).hexdigest(),
    }


def matrix_document() -> dict[str, Any]:
    return json.loads((ROOT / "eval/canonical-matrix.json").read_text())


def primary_schedule(repetitions: int) -> list[runner.RunItem]:
    experiments = set(runner.CONDITIONS) - runner.EXECUTABLE_CANDIDATE_EXPERIMENTS
    schedule = runner.build_schedule(experiments, seed=1729)
    return [item for item in schedule if item.run_index <= repetitions]


def candidate_schedule(repetitions: int) -> list[runner.RunItem]:
    schedule = []
    for experiment in sorted(runner.EXECUTABLE_CANDIDATE_EXPERIMENTS):
        base = runner.build_schedule({experiment}, seed=1729)
        conditions = {
            item.condition: item
            for item in base
            if item.run_index == 1
        }
        for run_index in range(1, repetitions + 1):
            schedule.extend(
                dataclasses.replace(item, run_index=run_index)
                for item in conditions.values()
            )
    return schedule


def required_outputs(experiment: str, matrix: dict[str, Any]) -> set[str]:
    definitions = (
        matrix["enhanced_candidate"]["experiments"]
        if experiment in runner.EXECUTABLE_CANDIDATE_EXPERIMENTS
        else matrix["experiments"]
    )
    return set(definitions[experiment].get("required_outputs", []))


def interval_bytes(item: runner.RunItem) -> int:
    return (math.ceil(item.measurement_secs) + 2) * INTERVAL_BYTES_PER_ROW


def estimate_files_and_bytes(
    schedule: list[runner.RunItem],
    *,
    host_sessions: int,
    include_enhanced_outputs: bool,
    matrix: dict[str, Any],
) -> dict[str, int | float | bool]:
    physical = [item for item in schedule if item.shared_from is None]
    aliases = len(schedule) - len(physical)
    raw_files = 0
    interval_files = 0
    interval_output_bytes = 0
    for item in physical:
        outputs = required_outputs(item.experiment, matrix)
        raw_files += len(COMMON_LEAF_FILES | outputs)
        if "interval-metrics.json" in outputs:
            interval_files += 1
            interval_output_bytes += interval_bytes(item)
    raw_files += host_sessions * HOST_SESSION_FILES
    raw_bytes = (
        len(physical) * HISTORICAL_BYTES_PER_LEAF
        + interval_output_bytes
        + host_sessions * (HOST_USB_CORPUS_BYTES + HOST_SESSION_OTHER_BYTES)
    )
    alias_bytes = aliases * ALIAS_RECEIPT_BYTES
    candidate_summary_files = (
        len(CANDIDATE_SUMMARY_FILES) if include_enhanced_outputs else 0
    )
    manifest_files = CORE_MANIFEST_FILES + candidate_summary_files + aliases
    manifest_bytes = CORE_MANIFEST_BYTES + alias_bytes
    derived_files = CANONICAL_DERIVED_FILES + (
        ENHANCED_DERIVED_FILES if include_enhanced_outputs else 0
    )
    derived_bytes = CANONICAL_DERIVED_BYTES + (
        ENHANCED_DERIVED_BYTES if include_enhanced_outputs else 0
    )
    reserve_bytes = math.ceil(
        raw_bytes * INTERRUPTED_ATTEMPT_RESERVE_PERCENT / 100
    )
    reserve_files = math.ceil(
        raw_files * INTERRUPTED_ATTEMPT_RESERVE_PERCENT / 100
    )
    estimated_bytes = raw_bytes + manifest_bytes + derived_bytes + reserve_bytes
    estimated_files = raw_files + manifest_files + derived_files + reserve_files
    required_free_bytes = 2 * estimated_bytes
    return {
        "estimated_file_count": estimated_files,
        "raw_file_count": raw_files,
        "interval_file_count": interval_files,
        "alias_receipt_file_count": aliases,
        "manifest_file_count": manifest_files,
        "derived_report_file_count": derived_files,
        "interrupted_attempt_reserve_file_count": reserve_files,
        "estimated_bytes": estimated_bytes,
        "raw_bytes": raw_bytes,
        "interval_output_bytes": interval_output_bytes,
        "alias_receipt_bytes": alias_bytes,
        "manifest_bytes": manifest_bytes,
        "derived_report_bytes": derived_bytes,
        "interrupted_attempt_reserve_bytes": reserve_bytes,
        "required_free_bytes_for_2x_margin": required_free_bytes,
        "usb_capacity_bytes": USB_CAPACITY_BYTES,
        "usb_capacity_gib": USB_CAPACITY_BYTES / GIB,
        "estimated_gib": estimated_bytes / GIB,
        "required_free_gib_for_2x_margin": required_free_bytes / GIB,
        "capacity_to_estimate_ratio": USB_CAPACITY_BYTES / estimated_bytes,
        "fits_with_2x_margin": required_free_bytes <= USB_CAPACITY_BYTES,
    }


def estimate_scenario(
    name: str,
    schedule: list[runner.RunItem],
    *,
    host_sessions: int,
    include_enhanced_outputs: bool,
    matrix: dict[str, Any],
) -> dict[str, Any]:
    aliases = sum(item.shared_from is not None for item in schedule)
    measured_leaves = len(schedule) - aliases + host_sessions
    nominal_seconds = sum(
        item.warmup_secs
        + item.measurement_secs
        + (60 if item.experiment == runner.CAPACITY_KNEE_EXPERIMENT else 0)
        for item in schedule
        if item.shared_from is None
    ) + host_sessions * HOST_DURATION_SECONDS
    result = {
        "name": name,
        "schedule_records": len(schedule) + host_sessions,
        "measured_or_static_leaves": measured_leaves,
        "shared_aliases": aliases,
        "host_sessions": host_sessions,
        "nominal_seconds": nominal_seconds,
        "nominal_hours": nominal_seconds / 3600,
        "operational_hours_10_percent": nominal_seconds * 1.1 / 3600,
        "experiment_record_counts": dict(
            sorted(Counter(item.experiment for item in schedule).items())
        ),
    }
    if host_sessions:
        result["experiment_record_counts"]["e-host-thermal-storage"] = host_sessions
    result.update(
        estimate_files_and_bytes(
            schedule,
            host_sessions=host_sessions,
            include_enhanced_outputs=include_enhanced_outputs,
            matrix=matrix,
        )
    )
    return result


def runner_estimates(matrix: dict[str, Any]) -> dict[str, dict[str, Any]]:
    expanded_n5 = primary_schedule(5) + candidate_schedule(5)
    all_candidate_n30 = primary_schedule(30) + candidate_schedule(30)
    return {
        "expanded-n5": estimate_scenario(
            "expanded-n5",
            expanded_n5,
            host_sessions=1,
            include_enhanced_outputs=True,
            matrix=matrix,
        ),
        "all-candidate-n30": estimate_scenario(
            "all-candidate-n30",
            all_candidate_n30,
            host_sessions=1,
            include_enhanced_outputs=True,
            matrix=matrix,
        ),
        "primary-only-n30": estimate_scenario(
            "primary-only-n30",
            primary_schedule(30),
            host_sessions=0,
            include_enhanced_outputs=False,
            matrix=matrix,
        ),
    }


def definition_cells(experiment: str, definition: dict[str, Any]) -> int:
    if experiment == runner.CAPACITY_KNEE_EXPERIMENT:
        return sum(len(rates) for rates in definition["condition_grid_msg_s"].values())
    if experiment == runner.PAYLOAD_REFINEMENT_EXPERIMENT:
        return len(definition["payload_bytes"])
    if experiment == runner.DEPTH_EXTENSION_EXPERIMENT:
        return len(definition["depths"])
    if experiment == runner.EKUIPER_PROFILE_EXPERIMENT:
        return len(definition["rates_msg_s"]) * len(definition["profiler_states"])
    return len(definition["conditions"])


def matrix_recomputation(
    matrix: dict[str, Any], *, primary_repetitions: int, candidate_repetitions: int | None
) -> dict[str, int]:
    records = 0
    aliases = 0
    nominal_seconds = 0
    for experiment, definition in matrix["experiments"].items():
        repetitions = min(int(definition["repetitions"]), primary_repetitions)
        cells = len(definition["conditions"])
        if experiment == "e-perf-10":
            cells *= len(definition["rate_points_msg_s"])
        count = repetitions * cells
        records += count
        if experiment in runner.CANONICAL_ALIASES:
            aliases += count
        else:
            nominal_seconds += count * (
                int(definition.get("warmup_secs", 0))
                + int(definition.get("measurement_secs", 0))
            )
    host_sessions = 0
    if candidate_repetitions is not None:
        for experiment, definition in matrix["enhanced_candidate"]["experiments"].items():
            if experiment == "e-host-thermal-storage":
                host_sessions = 1
                continue
            cells = definition_cells(experiment, definition)
            count = candidate_repetitions * cells
            records += count
            nominal_seconds += count * (
                int(definition.get("warmup_secs", 0))
                + int(definition.get("measurement_secs", 0))
                + (60 if experiment == runner.CAPACITY_KNEE_EXPERIMENT else 0)
            )
        records += host_sessions
        nominal_seconds += host_sessions * HOST_DURATION_SECONDS
    return {
        "schedule_records": records,
        "measured_or_static_leaves": records - aliases,
        "shared_aliases": aliases,
        "host_sessions": host_sessions,
        "nominal_seconds": nominal_seconds,
    }


def independent_estimates(matrix: dict[str, Any]) -> dict[str, dict[str, int]]:
    return {
        "expanded-n5": matrix_recomputation(
            matrix, primary_repetitions=5, candidate_repetitions=5
        ),
        "all-candidate-n30": matrix_recomputation(
            matrix, primary_repetitions=30, candidate_repetitions=30
        ),
        "primary-only-n30": matrix_recomputation(
            matrix, primary_repetitions=30, candidate_repetitions=None
        ),
    }


def accounting_fields(value: dict[str, Any]) -> dict[str, int]:
    return {
        key: int(value[key])
        for key in (
            "schedule_records",
            "measured_or_static_leaves",
            "shared_aliases",
            "host_sessions",
            "nominal_seconds",
        )
    }


def verify_match(
    estimates: dict[str, dict[str, Any]], independent: dict[str, dict[str, int]]
) -> None:
    if set(estimates) != set(independent):
        raise ValueError("schedule estimate scenario sets differ")
    for name in estimates:
        if accounting_fields(estimates[name]) != independent[name]:
            raise ValueError(f"independent schedule recomputation differs for {name}")


def receipt(
    kind: str,
    scenarios: dict[str, Any],
    tcc_root: Path,
    *,
    generated_at: str | None = None,
) -> dict[str, Any]:
    wafer_state = git_state(ROOT)
    tcc_state = git_state(tcc_root)
    return {
        "schema_version": 1,
        "kind": kind,
        "generated_at": generated_at or dt.datetime.now(dt.UTC).isoformat(),
        "provisional": wafer_state["dirty"] or tcc_state["dirty"],
        "source": {
            "wafer": wafer_state,
            "tcc_doc": tcc_state,
            "canonical_matrix_sha256": sha256(ROOT / "eval/canonical-matrix.json"),
        },
        "assumptions": {
            "usb_capacity_bytes": USB_CAPACITY_BYTES,
            "historical_bytes_per_leaf": HISTORICAL_BYTES_PER_LEAF,
            "interval_bytes_per_row": INTERVAL_BYTES_PER_ROW,
            "alias_receipt_bytes": ALIAS_RECEIPT_BYTES,
            "interrupted_attempt_reserve_percent": INTERRUPTED_ATTEMPT_RESERVE_PERCENT,
            "host_usb_corpus_bytes": HOST_USB_CORPUS_BYTES,
            "host_session_other_bytes": HOST_SESSION_OTHER_BYTES,
            "canonical_derived_bytes": CANONICAL_DERIVED_BYTES,
            "enhanced_derived_bytes": ENHANCED_DERIVED_BYTES,
        },
        "scenarios": scenarios,
        "campaign_started": False,
    }


def write_json(path: Path, value: dict[str, Any]) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(json.dumps(value, indent=2, sort_keys=True) + "\n")


def estimate(args: argparse.Namespace) -> None:
    matrix = matrix_document()
    estimates = runner_estimates(matrix)
    independent = independent_estimates(matrix)
    verify_match(estimates, independent)
    write_json(args.output, receipt("runner-derived", estimates, args.tcc_root))
    write_json(
        args.independent_output,
        receipt("matrix-independent-recomputation", independent, args.tcc_root),
    )
    print(
        "schedule estimates: PASS "
        + ", ".join(
            f"{name}={value['schedule_records']} records/{value['estimated_gib']:.2f} GiB"
            for name, value in estimates.items()
        )
    )


def verify(args: argparse.Namespace) -> None:
    observed = json.loads(args.output.read_text())
    independent_observed = json.loads(args.independent_output.read_text())
    matrix = matrix_document()
    try:
        dt.datetime.fromisoformat(observed["generated_at"])
        dt.datetime.fromisoformat(independent_observed["generated_at"])
    except (KeyError, TypeError, ValueError) as error:
        raise ValueError("schedule estimate receipt has invalid generated_at") from error
    expected = receipt(
        "runner-derived",
        runner_estimates(matrix),
        args.tcc_root,
        generated_at=observed["generated_at"],
    )
    independent_expected = receipt(
        "matrix-independent-recomputation",
        independent_estimates(matrix),
        args.tcc_root,
        generated_at=independent_observed["generated_at"],
    )
    if observed != expected or independent_observed != independent_expected:
        raise ValueError("schedule estimate receipts differ from current source state")
    verify_match(observed["scenarios"], independent_observed["scenarios"])
    if not observed["scenarios"]["expanded-n5"]["fits_with_2x_margin"]:
        raise ValueError("expanded N=5 does not fit the USB with 2x margin")
    print("schedule estimate verification: PASS (independent recomputation matched)")


def parser() -> argparse.ArgumentParser:
    value = argparse.ArgumentParser()
    subparsers = value.add_subparsers(dest="command", required=True)
    for name, handler in (("estimate", estimate), ("verify", verify)):
        command = subparsers.add_parser(name)
        command.add_argument("--output", type=Path, required=True)
        command.add_argument("--independent-output", type=Path, required=True)
        command.add_argument(
            "--tcc-root",
            type=Path,
            default=Path("/Users/i572543/Dev/github.com/PedroKlein/tcc-doc/main"),
        )
        command.set_defaults(handler=handler)
    return value


def main() -> int:
    args = parser().parse_args()
    try:
        args.handler(args)
    except (OSError, ValueError, subprocess.CalledProcessError) as error:
        print(f"error: {error}", file=sys.stderr)
        return 1
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
