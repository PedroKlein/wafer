"""Check that result leaves were produced from the same source and plugin bytes.

The canonical matrix lives in the repository and canonical leaves must come
from a clean tree, so an equal `git_sha` also means an equal matrix.
"""

from __future__ import annotations

import json
from pathlib import Path


def leaf_provenance(leaf: Path) -> dict[str, str]:
    """The comparable fields of one leaf, flattened to `field -> value`."""
    metadata = json.loads((leaf / "metadata.json").read_text())
    fields: dict[str, str] = {}
    sha = metadata.get("git_sha")
    if isinstance(sha, str):
        fields["git_sha"] = sha
    plugins = metadata.get("wafer_plugin_hashes")
    if isinstance(plugins, dict):
        for name, digest in plugins.items():
            fields[f"wafer_plugin_hashes.{name}"] = str(digest)
    return fields


def provenance_mismatches(leaves: list[Path]) -> list[tuple[Path, str]]:
    """One violation per field, naming the first leaf that differs from the first leaf that set it."""
    reference: dict[str, tuple[str, Path]] = {}
    reported: set[str] = set()
    violations: list[tuple[Path, str]] = []
    for leaf in leaves:
        try:
            fields = leaf_provenance(leaf)
        except (OSError, ValueError) as error:
            violations.append((leaf, f"cannot read provenance: {error}"))
            continue
        for field, value in fields.items():
            if field not in reference:
                reference[field] = (value, leaf)
                continue
            expected, origin = reference[field]
            if value != expected and field not in reported:
                reported.add(field)
                violations.append(
                    (leaf, f"{field} is {value}, but {origin} has {expected}")
                )
    return violations
