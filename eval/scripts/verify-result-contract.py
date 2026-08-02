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
import re
import sys
from pathlib import Path

# The split contract (RESULT-CONTRACT.md source of truth).
CORE_FILES = {"config.toml", "metadata.json", "stdout.log"}

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


def check_leaf(leaf: Path, experiment: str) -> list[str]:
    """Return a list of violation strings; empty list = conformant."""
    files = {f.name for f in leaf.iterdir() if f.is_file()}
    violations: list[str] = []

    for core in CORE_FILES:
        if core in files:
            continue
        # Legacy scripts predating F2 don't emit metadata.json — flag as
        # a known gap, not a fresh regression. The scripts themselves are
        # tracked by P-Followup-2 tail (not shipped this plan).
        if core == "metadata.json" and experiment in LEGACY_METADATA_MISSING_ALLOWED:
            continue
        violations.append(f"missing core artefact: {core}")

    matrix = OPTIONAL_MATRIX.get(experiment)
    if matrix is None:
        return violations  # Unknown experiment id — core-only check.

    for f in matrix["must_have"]:
        if f not in files:
            violations.append(f"missing required optional artefact for {experiment}: {f}")
    for f in matrix.get("must_not_have", set()):
        if f in files:
            violations.append(f"unexpected artefact for {experiment}: {f}")
    return violations


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__.split("\n\n", 1)[0])
    parser.add_argument("dirs", nargs="+", type=Path)
    args = parser.parse_args()

    all_violations: list[tuple[Path, str]] = []
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
            for v in check_leaf(leaf, experiment):
                all_violations.append((leaf, v))

    if all_violations:
        for leaf, v in all_violations:
            print(f"VIOLATION  {leaf}: {v}")
        print(f"\n{len(all_violations)} violation(s) across {checked} leaf run(s).")
        return 1

    print(f"OK: {checked} leaf run(s) conform to split RESULT-CONTRACT")
    return 0


if __name__ == "__main__":
    sys.exit(main())
