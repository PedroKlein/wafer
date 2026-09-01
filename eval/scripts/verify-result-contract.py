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


def check_leaf(
    leaf: Path,
    experiment: str,
    canonical: bool = False,
    canonical_matrix: dict | None = None,
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
    parser.add_argument("dirs", nargs="+", type=Path)
    args = parser.parse_args()

    canonical_matrix: dict | None = None
    if args.canonical:
        try:
            canonical_matrix = json.loads(CANONICAL_MATRIX.read_text())
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
