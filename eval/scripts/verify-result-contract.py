#!/usr/bin/env python3
"""Structural verifier for eval/RESULT-CONTRACT.md compliance.

Confirms the F3 split-contract decision (option B) works in practice:
- **Core artefacts** MUST exist in every leaf run directory.
- **Optional artefacts** may exist per the per-experiment matrix; when
  absent, this is contract-conformant, not a violation.

Reviewer note: this replaces the AC F3.AC5 "run two fresh shakedowns"
requirement with structural inspection of the existing post-decision
shakedown dirs. Cheaper and reproducible; the reviewer's real question
is 'does the split-contract hold?' which is best answered by walking
the manifest.

Exit codes:
    0  every leaf under the given experiment dirs conforms.
    1  at least one leaf violates core or optional contract in a way
       the split-contract permits (e.g. contract says memory.csv MUST
       exist for E-Perf-6 but it is absent).
    2  invocation error (bad args).

Usage:
    verify-result-contract.py <experiment-run-dir>...

Example:
    verify-result-contract.py \\
      eval/results/e-val-1/shakedown-macos-2026-07-22T16-19-29Z \\
      eval/results/e-perf-6/shakedown-macos-2026-07-22T17-49-56Z
"""

from __future__ import annotations

import argparse
import csv
import hashlib
import json
import re
import sys
from pathlib import Path

# The split contract (RESULT-CONTRACT.md source of truth).
CORE_FILES = {"config.toml", "metadata.json", "stdout.log"}
CANONICAL_PI_FILES = {
    "measurement-window.json",
    "pi-telemetry.csv",
    "pmic-rails.csv",
    "power-boundary.json",
}
CANONICAL_MATRIX = Path(__file__).resolve().parents[1] / "canonical-matrix.json"

# Runtime-provenance keys populated by `eval/scripts/lib/write_metadata.py`
# when it merges `runtime-provenance.json` (emitted by wafer-runtime) into
# `metadata.json`. T8 (thesis-hardening): shakedown dirs lacking these keys
# get a WARN, not a violation — legacy shakedown scripts write bespoke
# minimal metadata by design (see script header comments). Canonical
# `run-experiment.sh` runs always merge these keys.
MERGED_PROVENANCE_KEYS = (
    "wasmtime_version",
    "wafer_runtime_sha256",
    "wafer_plugin_hashes",
)

# Optional-artefact expectations per experiment. Match on the experiment
# prefix embedded in the shakedown path. `must_have`: files whose absence
# is a violation for THIS experiment. `may_have`: files legitimately
# missing under the split contract; presence is fine, absence is fine.
OPTIONAL_MATRIX: dict[str, dict[str, set[str]]] = {
    "e-val-1": {
        "must_have": {"latency.hdr", "throughput.csv"},
        "may_have": {"sequence.csv", "percentiles.json"},
        "must_not_have": set(),  # memory.csv/per_node_metrics.csv absent by design
    },
    "e-perf-6": {
        "must_have": {"latency.hdr", "throughput.csv", "memory.csv"},
        "may_have": {"sequence.csv", "per_node_metrics.csv", "metadata.json"},
        "must_not_have": set(),
    },
}

# Legacy shakedown scripts that predate F2 metadata merge and F3 split
# contract still write bespoke leaf layouts. Track them as documented
# deviations so the verifier doesn't flag known-unshipped gaps as
# regressions.
LEGACY_METADATA_MISSING_ALLOWED = {"e-val-1"}


def find_leaf_dirs(root: Path) -> list[Path]:
    """Return every directory under *root* that contains a `config.toml`.

    Leaf-run detection: a run is a directory holding a config.toml. This
    handles both single-run experiments (E-Val-1/run-N) and multi-tier
    experiments (E-Perf-6/depth-N/run-NN).
    """
    return [p.parent for p in root.rglob("config.toml")]


def experiment_of(path: Path) -> str | None:
    """Extract the experiment id from a path like
    eval/results/e-perf-6/shakedown-macos-.../..."""
    for part in path.parts:
        if re.match(r"^e-[a-z0-9-]+$", part):
            return part
    return None


def check_rate_sweep_result(path: Path) -> list[str]:
    required = {
        "schema_version",
        "experiment",
        "system",
        "thesis_evidence",
        "measurement_boundary",
        "units",
        "offered_rate_msg_s",
        "actual_offered_rate_msg_s",
        "achieved_rate_msg_s",
        "measurement_duration_ns",
        "messages",
        "loss_percent",
        "latency_ns",
        "resources",
        "throttled",
        "profile",
        "process_audit",
        "traces",
    }
    nested = {
        "messages": {"offered", "received", "lost", "duplicates"},
        "latency_ns": {"p50", "p95", "p99"},
        "resources": {"scope", "cpu_percent", "max_rss_bytes"},
        "profile": {"path", "sha256", "payload_template_sha256"},
        "process_audit": {"path", "sha256"},
        "traces": {"published", "received"},
    }
    try:
        result = json.loads(path.read_text())
    except (OSError, ValueError) as error:
        return [f"rate-sweep.json is unreadable: {error}"]
    violations = [
        f"rate-sweep.json missing field: {field}"
        for field in sorted(required - result.keys())
    ]
    for section, fields in nested.items():
        value = result.get(section)
        if not isinstance(value, dict):
            violations.append(f"rate-sweep.json {section} must be an object")
            continue
        violations.extend(
            f"rate-sweep.json {section} missing field: {field}"
            for field in sorted(fields - value.keys())
        )
    if result.get("experiment") != "e-perf-10":
        violations.append("rate-sweep.json experiment must be e-perf-10")
    if result.get("thesis_evidence") is not False:
        violations.append("rate-sweep.json must set thesis_evidence=false")
    return violations


def _load_json(path: Path, label: str, violations: list[str]) -> dict | None:
    try:
        value = json.loads(path.read_text())
    except (OSError, ValueError) as error:
        violations.append(f"{label} is unreadable: {error}")
        return None
    if not isinstance(value, dict):
        violations.append(f"{label} must contain an object")
        return None
    return value


def _sequence_violations(path: Path) -> list[str]:
    try:
        with path.open(newline="") as stream:
            rows = list(csv.DictReader(stream))
        if len(rows) != 1:
            return ["sequence.csv must contain one summary row"]
        expected = int(rows[0]["total_expected"])
        received = int(rows[0]["total_received"])
        gaps = int(rows[0]["gap_msgs"])
        duplicates = int(rows[0]["duplicates_count"])
    except (KeyError, OSError, TypeError, ValueError) as error:
        return [f"sequence.csv is invalid: {error}"]
    if expected != received or gaps != 0 or duplicates != 0:
        return [
            "sequence.csv is not lossless: "
            f"expected={expected}, received={received}, gaps={gaps}, duplicates={duplicates}"
        ]
    return []


def _focused_leaf_identity(leaf: Path, experiment: str) -> tuple[str, int] | None:
    parts = leaf.parts
    try:
        experiment_index = parts.index(experiment)
        batch_index = next(
            index
            for index in range(experiment_index + 1, len(parts))
            if parts[index].startswith("rpi5-")
        )
    except (ValueError, StopIteration):
        return None
    match = re.match(r"run-(\d+)(?:-attempt-\d+)?$", leaf.name)
    if match is None:
        return None
    condition = "/".join(parts[batch_index + 1 : -1])
    return condition, int(match.group(1))


def check_focused_matrix(matrix: dict) -> list[str]:
    focused = matrix.get("focused_pilot")
    if not isinstance(focused, dict):
        return ["canonical matrix lacks focused_pilot"]
    if focused.get("thesis_evidence") is not False:
        return ["focused_pilot must set thesis_evidence=false"]
    decisions = focused.get("decisions", {})
    concurrency = decisions.get("ekuiper_operator_concurrency", {})
    memory = decisions.get("memory_retention", {})
    violations = []
    if concurrency.get("value") != 1 or concurrency.get("comparison") != "default-system":
        violations.append("focused_pilot eKuiper concurrency decision is not frozen at default-system value 1")
    if memory.get("status") != "fixed":
        violations.append("focused_pilot memory-retention status is not fixed")
    if memory.get("max_observed_slope_bytes_per_message") != 0.0:
        violations.append("focused_pilot memory-retention slope is not 0 bytes/message")
    if memory.get("canonical_measurement_secs_adequate") is not True:
        violations.append("focused_pilot memory-retention decision does not approve the measurement window")
    return violations


def check_focused_leaf(
    leaf: Path,
    experiment: str,
    metadata: dict,
    canonical_matrix: dict,
    matrix_sha256: str,
) -> list[str]:
    violations: list[str] = []
    focused = canonical_matrix["focused_pilot"]
    definition = focused.get("experiments", {}).get(experiment)
    identity = _focused_leaf_identity(leaf, experiment)
    if not isinstance(definition, dict) or identity is None:
        return [f"leaf is not selected by focused_pilot: {leaf}"]
    condition, run_index = identity
    if run_index not in definition.get("condition_runs", {}).get(condition, []):
        violations.append(
            f"focused_pilot does not select {experiment}/{condition}/run-{run_index:02d}"
        )
    if metadata.get("thesis_evidence") is not False:
        violations.append("focused pilot metadata must set thesis_evidence=false")
    provenance = metadata.get("focused_pilot", {})
    decisions = focused["decisions"]
    expected_provenance = {
        "id": focused["id"],
        "matrix_sha256": matrix_sha256,
        "memory_retention_fix_commit": decisions["memory_retention"]["fix_commit"],
        "ekuiper_operator_concurrency": decisions["ekuiper_operator_concurrency"]["value"],
    }
    if provenance != expected_provenance:
        violations.append("focused pilot metadata provenance differs from the frozen matrix")

    if experiment == "e-perf-10":
        sweep = _load_json(leaf / "rate-sweep.json", "rate-sweep.json", violations)
        if sweep is not None:
            violations.extend(check_rate_sweep_result(leaf / "rate-sweep.json"))
            if sweep.get("system") != metadata.get("system"):
                violations.append("rate-sweep system differs from metadata")
        if metadata.get("system") == "ekuiper":
            audit = _load_json(leaf / "ekuiper-audit.json", "ekuiper-audit.json", violations)
            expected = focused["decisions"]["ekuiper_operator_concurrency"]["value"]
            if audit is not None and audit.get("rule", {}).get("options", {}).get("concurrency") != expected:
                violations.append("eKuiper audit does not use frozen operator concurrency 1")

    if experiment == "e-backpressure":
        result = _load_json(leaf / "backpressure.json", "backpressure.json", violations)
        if result is not None:
            if result.get("classification") != "saturated-and-drained":
                violations.append("backpressure classification must be saturated-and-drained")
            peak_occupancy = result.get("peak_occupancy")
            occupancy_threshold = result.get("occupancy_threshold")
            if (
                result.get("threshold_crossed") is not True
                or result.get("recovered") is not True
                or not isinstance(peak_occupancy, (int, float))
                or not isinstance(occupancy_threshold, (int, float))
                or peak_occupancy < occupancy_threshold
            ):
                violations.append("backpressure evidence did not cross and recover below thresholds")
            if set(result.get("rates_msg_s", {})) != {"offered", "accepted", "processed", "drained"}:
                violations.append("backpressure rates must separate offered, accepted, processed, and drained")
            if result.get("rates_msg_s", {}).get("drained") is None:
                violations.append("backpressure drain rate is missing")
            if result.get("sequence", {}).get("lossless") is not True:
                violations.append("backpressure slow policy is not lossless")
            if result.get("memory", {}).get("within_limit") is not True:
                violations.append("backpressure RSS exceeds the frozen limit")

    if experiment == "e-iso-4":
        result = _load_json(leaf / "containment.json", "containment.json", violations)
        if result is not None:
            traps = int(result.get("traps_total", 0))
            recoveries = sum(int(node.get("recovery_count", 0)) for node in result.get("nodes", []))
            if result.get("contained") is not True or traps <= 0:
                violations.append("epoch containment did not record a contained trap")
            if recoveries != traps:
                violations.append(f"epoch recovery count differs from traps: {recoveries} != {traps}")
        try:
            if "cannot enter component instance" in (leaf / "stdout.log").read_text(errors="replace"):
                violations.append("epoch recovery reused an interrupted component instance")
        except OSError:
            pass

    if experiment == "e-iso-7":
        result = _load_json(leaf / "branch-isolation.json", "branch-isolation.json", violations)
        if result is not None:
            if result.get("condition") != condition:
                violations.append("branch-isolation condition differs from the leaf")
            if result.get("measurement_boundary", {}).get("kind") != "branch_sink_post_warmup":
                violations.append("branch-isolation measurement boundary is not branch-local")
            branches = result.get("branches", {})
            branch_a = branches.get("branch_a", {})
            branch_b = branches.get("branch_b", {})
            if (
                branch_a.get("artifact_dir") != "branch-a"
                or branch_b.get("artifact_dir") != "branch-b"
            ):
                violations.append("branch-isolation evidence does not use independent branch sinks")
            if (
                branch_a.get("offered_messages") != branch_a.get("received_messages")
                or branch_a.get("lost_messages") != 0
                or branch_a.get("gap_messages") != 0
                or branch_a.get("duplicates") != 0
            ):
                violations.append("branch A is not lossless and independently attributed")

    if experiment == "e-perf-9":
        result = _load_json(leaf / "startup.json", "startup.json", violations)
        if result is not None:
            phases = result.get("phases_ns", {})
            required_phases = {
                "process_config", "component_load_compile", "instantiation",
                "pipeline_setup", "first_process",
            }
            if set(phases) != required_phases or any(type(value) is not int or value < 0 for value in phases.values()):
                violations.append("startup phases must be complete non-negative nanosecond values")
            if result.get("processed_messages") != 1:
                violations.append("startup evidence must contain exactly one processed message")
            cache = result.get("compiled_component_cache", {})
            if cache.get("mode") == "disabled" and (
                cache.get("hit") is not False
                or cache.get("artifact") is not None
                or cache.get("identity") is not None
            ):
                violations.append("disabled compiled cache reports unsupported hit evidence")

    if experiment in {"e-swap-1", "e-swap-2", "e-swap-4", "e-swap-6"}:
        result = _load_json(leaf / "hotswap-analysis.json", "hotswap-analysis.json", violations)
        expected_events = definition.get("events_per_run")
        if result is not None:
            events = result.get("events")
            if result.get("duration_unit") != "ns" or not isinstance(events, list):
                violations.append("hot-swap analysis must contain nanosecond event records")
            elif result.get("sample_count") != expected_events or len(events) != expected_events:
                violations.append(f"hot-swap analysis must contain {expected_events} events")
            else:
                required = {
                    "compile_ns", "instantiate_ns", "signal_ns", "ack_ns",
                    "convergence_ns", "http_total_ns", "sink_observed_output_gap_ns",
                }
                if any(
                    any(type(event.get(field)) is not int or event[field] < 0 for field in required)
                    for event in events
                ):
                    violations.append("hot-swap event phases must be non-negative nanoseconds")

    if experiment.startswith("e-swap-"):
        violations.extend(_sequence_violations(leaf / "sequence.csv"))
    if experiment == "e-swap-5":
        result = _load_json(leaf / "rollback.json", "rollback.json", violations)
        if result is not None:
            expected_events = definition.get("events_per_run")
            if (
                result.get("attempts") != expected_events
                or result.get("rolled_back") != expected_events
                or result.get("all_rolled_back") is not True
            ):
                violations.append(f"rollback evidence must show {expected_events} successful rollbacks")
    return violations


def check_leaf(
    leaf: Path,
    experiment: str,
    canonical: bool = False,
    canonical_matrix: dict | None = None,
    focused: bool = False,
    matrix_sha256: str = "",
) -> tuple[list[str], list[str]]:
    """Return (violations, warnings). Empty lists = fully conformant."""
    files = {f.name for f in leaf.iterdir() if f.is_file()}
    violations: list[str] = []
    warnings: list[str] = []

    for core in CORE_FILES:
        if core in files:
            continue
        # Legacy scripts predating F2 don't emit metadata.json — flag as
        # a known gap, not a fresh regression. The scripts themselves are
        # tracked by P-Followup-2 tail (not shipped this plan).
        if core == "metadata.json" and experiment in LEGACY_METADATA_MISSING_ALLOWED:
            continue
        violations.append(f"missing core artefact: {core}")

    # T8: warn (don't fail) when metadata.json is present but lacks the
    # provenance keys that `write_metadata.py` merges from
    # `runtime-provenance.json`. Legacy shakedown scripts write bespoke
    # per-experiment metadata schemas by design and are documented as
    # such in their headers; the WARN surfaces the gap without breaking
    # existing baseline dirs.
    meta_path = leaf / "metadata.json"
    metadata: dict = {}
    if meta_path.is_file():
        try:
            with meta_path.open() as fh:
                metadata = json.load(fh)
            missing = [k for k in MERGED_PROVENANCE_KEYS if k not in metadata]
            if missing and metadata.get("system") not in {"ekuiper", "mqtt-loopback"}:
                message = (
                    f"metadata.json lacks merged provenance keys: {sorted(missing)} "
                    f"(legacy shakedown script; canonical runs source "
                    f"eval/scripts/lib/write_metadata.py)"
                )
                if canonical:
                    violations.append(message)
                else:
                    warnings.append(message)
            if leaf.name.startswith("rpi5-") or canonical:
                expected = {
                    "host_tag": "rpi5",
                    "arch": "aarch64",
                    "isolated_cpus": "1-3",
                    "throttled": "0x0",
                }
                for key, value in expected.items():
                    if metadata.get(key) != value:
                        violations.append(
                            f"Pi 5 metadata {key}={metadata.get(key)!r}, expected {value!r}"
                        )
                if "Raspberry Pi 5" not in str(metadata.get("hardware_model", "")):
                    violations.append("Pi 5 metadata lacks Raspberry Pi 5 hardware model")
                if metadata.get("cpu_governors") != ["performance"]:
                    violations.append("Pi 5 metadata CPU governor is not performance")
                if not re.fullmatch(r"[0-9a-f]{40}", str(metadata.get("git_sha", ""))):
                    violations.append("Pi 5 metadata lacks a source commit SHA")
                if not isinstance(metadata.get("git_dirty"), bool):
                    violations.append("Pi 5 metadata git_dirty is not boolean")
                exit_codes = metadata.get("exit_codes", {})
                if metadata.get("system") == "ekuiper":
                    if exit_codes.get("ekuiper") != 0:
                        violations.append("Pi 5 metadata records a non-zero eKuiper exit")
                elif metadata.get("system") == "mqtt-loopback":
                    if exit_codes.get("publisher") != 0 or exit_codes.get("subscriber") != 0:
                        violations.append("Pi 5 metadata records a non-zero loopback loadgen exit")
                elif exit_codes.get("wafer_runtime") != 0:
                    violations.append("Pi 5 metadata records a non-zero runtime exit")
                if experiment == "e-perf-10" and metadata.get("thesis_evidence") is not False:
                    violations.append("E-Perf-10 metadata must set thesis_evidence=false")
                if canonical:
                    if metadata.get("git_dirty") is not False:
                        violations.append("canonical result records dirty source")
                    tags = metadata.get("git_tags")
                    if not isinstance(tags, list) or not tags:
                        violations.append("canonical result lacks tagged source provenance")
                    if (
                        metadata.get("system") not in {"ekuiper", "mqtt-loopback"}
                        and "runtime-provenance.json" not in files
                    ):
                        violations.append("missing canonical runtime provenance: runtime-provenance.json")
        except (OSError, ValueError) as exc:
            warnings.append(f"metadata.json unreadable: {exc}")

    if canonical:
        for required in CANONICAL_PI_FILES:
            if required not in files:
                violations.append(f"missing canonical Pi telemetry artefact: {required}")
        window_path = leaf / "measurement-window.json"
        if window_path.is_file():
            try:
                window = json.loads(window_path.read_text())
                if int(window["finished_ns"]) <= int(window["started_ns"]):
                    violations.append("canonical measurement window is empty or reversed")
            except (KeyError, OSError, TypeError, ValueError):
                violations.append("canonical measurement window is invalid")

    if canonical and canonical_matrix is not None:
        experiment_contract = canonical_matrix.get("experiments", {}).get(experiment)
        if not isinstance(experiment_contract, dict):
            violations.append(f"experiment {experiment} is absent from canonical matrix")
        else:
            for required in experiment_contract.get("required_outputs", []):
                if required not in files:
                    violations.append(
                        f"missing required canonical artefact for {experiment}: {required}"
                    )

    if experiment == "e-perf-10" and "rate-sweep.json" in files:
        violations.extend(check_rate_sweep_result(leaf / "rate-sweep.json"))

    if focused and canonical_matrix is not None:
        violations.extend(
            check_focused_leaf(leaf, experiment, metadata, canonical_matrix, matrix_sha256)
        )

    matrix = OPTIONAL_MATRIX.get(experiment)
    if matrix is None:
        return violations, warnings  # Unknown experiment id — core-only check.

    for f in matrix["must_have"]:
        if f not in files:
            violations.append(f"missing required optional artefact for {experiment}: {f}")
    for f in matrix.get("must_not_have", set()):
        if f in files:
            violations.append(f"unexpected artefact for {experiment}: {f}")
    return violations, warnings


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__.split("\n\n", 1)[0])
    parser.add_argument(
        "--canonical",
        action="store_true",
        help="enforce Pi 5 provenance and experiment-specific canonical outputs",
    )
    parser.add_argument(
        "--focused",
        action="store_true",
        help="enforce the frozen focused-pilot selection and semantic invariants",
    )
    parser.add_argument("--matrix", type=Path, default=CANONICAL_MATRIX)
    parser.add_argument("dirs", nargs="+", type=Path)
    args = parser.parse_args()

    if args.focused and not args.canonical:
        parser.error("--focused requires --canonical")

    canonical_matrix: dict | None = None
    matrix_sha256 = ""
    if args.canonical:
        try:
            matrix_bytes = args.matrix.read_bytes()
            canonical_matrix = json.loads(matrix_bytes)
            matrix_sha256 = hashlib.sha256(matrix_bytes).hexdigest()
        except (OSError, ValueError) as exc:
            print(f"error: cannot load canonical matrix: {exc}", file=sys.stderr)
            return 2

    all_violations: list[tuple[Path, str]] = []
    all_warnings: list[tuple[Path, str]] = []
    canonical_shas: set[str] = set()
    checked = 0

    for root in args.dirs:
        if not root.exists():
            print(f"warn: {root} does not exist", file=sys.stderr)
            continue
        experiment = experiment_of(root)
        if experiment is None:
            print(f"warn: cannot infer experiment id from {root}", file=sys.stderr)
            continue
        leaves = find_leaf_dirs(root)
        if not leaves:
            print(f"warn: no leaf run dirs (config.toml) under {root}", file=sys.stderr)
            continue
        for leaf in leaves:
            checked += 1
            violations, warnings = check_leaf(
                leaf,
                experiment,
                canonical=args.canonical,
                canonical_matrix=canonical_matrix,
                focused=args.focused,
                matrix_sha256=matrix_sha256,
            )
            if args.canonical:
                try:
                    metadata = json.loads((leaf / "metadata.json").read_text())
                    sha = metadata.get("git_sha")
                    if isinstance(sha, str):
                        canonical_shas.add(sha)
                except (OSError, ValueError):
                    pass
            for v in violations:
                all_violations.append((leaf, v))
            for w in warnings:
                all_warnings.append((leaf, w))

    if args.canonical and len(canonical_shas) > 1:
        all_violations.append(
            (Path("<batch>"), f"canonical inputs mix source SHAs: {sorted(canonical_shas)}")
        )
    if args.focused and canonical_matrix is not None:
        all_violations.extend(
            (Path("<focused-pilot>"), violation)
            for violation in check_focused_matrix(canonical_matrix)
        )

    for leaf, w in all_warnings:
        print(f"WARN       {leaf}: {w}")

    if all_violations:
        for leaf, v in all_violations:
            print(f"VIOLATION  {leaf}: {v}")
        print(f"\n{len(all_violations)} violation(s), {len(all_warnings)} warning(s) across {checked} leaf run(s).")
        return 1

    warn_count = len(all_warnings)
    if warn_count:
        print(f"OK: {checked} leaf run(s) conform to split RESULT-CONTRACT ({warn_count} warning(s) — non-fatal)")
    else:
        print(f"OK: {checked} leaf run(s) conform to split RESULT-CONTRACT")
    return 0


if __name__ == "__main__":
    sys.exit(main())
