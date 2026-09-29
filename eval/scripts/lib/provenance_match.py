"""Check that result leaves were produced from the same source and plugin bytes.

The canonical matrix lives in the repository, so leaves from a clean tree at
one `git_sha` share one matrix. Plugin hashes are keyed by pipeline node id,
and one node id loads different plugins in different conditions, so they are
compared only between leaves of the same experiment and condition.
"""

from __future__ import annotations

import json
import re
from pathlib import Path

RUN_DIR = re.compile(r"run-\d+(?:-attempt-\d+)?")
EXPERIMENT_DIR = re.compile(r"e-[a-z0-9-]+")


def condition_key(leaf: Path) -> str:
    """`experiment/condition...` for a leaf, without the batch and run directories."""
    parts = leaf.parts
    starts = [index for index, part in enumerate(parts) if EXPERIMENT_DIR.fullmatch(part)]
    if not starts:
        return str(leaf)
    experiment = starts[-1]
    tail = list(parts[experiment + 2 :])
    if tail and RUN_DIR.fullmatch(tail[-1]):
        tail.pop()
    return "/".join([parts[experiment], *tail])


def provenance_mismatches(leaves: list[Path]) -> list[tuple[Path, str]]:
    """One violation per differing field, naming the leaf and the leaf it was compared with."""
    violations: list[tuple[Path, str]] = []
    source: tuple[str, Path] | None = None
    plugins: dict[str, tuple[dict, Path]] = {}
    reported: set[str] = set()

    def report(leaf: Path, field: str, message: str) -> None:
        if field not in reported:
            reported.add(field)
            violations.append((leaf, message))

    for leaf in leaves:
        try:
            metadata = json.loads((leaf / "metadata.json").read_text())
        except (OSError, ValueError) as error:
            violations.append((leaf, f"cannot read provenance: {error}"))
            continue
        sha = metadata.get("git_sha")
        if not isinstance(sha, str) or not sha:
            violations.append((leaf, "provenance lacks git_sha"))
        elif source is None:
            source = (sha, leaf)
        elif sha != source[0]:
            report(leaf, "git_sha", f"git_sha is {sha}, but {source[1]} has {source[0]}")
        if metadata.get("git_dirty") is not False:
            violations.append((leaf, "provenance comes from a dirty or unrecorded source tree"))

        hashes = metadata.get("wafer_plugin_hashes")
        hashes = hashes if isinstance(hashes, dict) else {}
        key = condition_key(leaf)
        if key not in plugins:
            plugins[key] = (hashes, leaf)
            continue
        expected, origin = plugins[key]
        for node in sorted(expected.keys() | hashes.keys()):
            field = f"{key}:wafer_plugin_hashes.{node}"
            if node not in hashes:
                report(leaf, field, f"wafer_plugin_hashes lacks {node}, which {origin} has")
            elif node not in expected:
                report(leaf, field, f"wafer_plugin_hashes.{node} is absent from {origin}")
            elif hashes[node] != expected[node]:
                report(
                    leaf,
                    field,
                    f"wafer_plugin_hashes.{node} is {hashes[node]}, but {origin} has {expected[node]}",
                )
    return violations
