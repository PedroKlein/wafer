"""Path-discovery helpers for WAFER evaluation notebooks.

Centralises shakedown result-directory resolution so notebooks never
hardcode timestamps that rot as new runs land.
"""

from __future__ import annotations

import json
import os
import pathlib
from typing import Optional


def find_latest_shakedown(
    experiment_id: str,
    host_tag: str = "shakedown-macos",
    pinned: Optional[str] = None,
) -> pathlib.Path:
    """Return the newest shakedown result directory for *experiment_id*.

    Globs ``eval/results/{experiment_id}/{host_tag}-*`` relative to the
    repository root and returns the lexicographically last match.  Because
    timestamps are ISO-8601 UTC (``YYYY-MM-DDTHH-MM-SSZ``), lex sort equals
    time sort — no datetime parsing needed.

    Parameters
    ----------
    experiment_id:
        Experiment identifier, e.g. ``"e-perf-1"``.
    host_tag:
        Directory-name prefix before the timestamp. Default ``"shakedown-macos"``.
    pinned:
        When set, bypasses the glob and returns this exact path (resolved).
        Useful for notebooks that need reproducible figure output pinned to
        a specific run.

    Raises
    ------
    FileNotFoundError
        When no matching directories exist (and *pinned* is not set).
    """
    repo = _find_repo_root()

    if pinned is not None:
        path = pathlib.Path(pinned)
        if not path.is_absolute():
            path = repo / path
        return path.resolve()

    batch_id = os.environ.get("WAFER_EVAL_BATCH_ID")
    if batch_id:
        return find_canonical_batch(experiment_id, batch_id)

    pattern = f"{host_tag}-*"
    parent = repo / "eval" / "results" / experiment_id
    candidates = sorted(parent.glob(pattern))
    if not candidates:
        raise FileNotFoundError(
            f"No shakedown directories matching {parent / pattern}"
        )
    return candidates[-1]


def find_canonical_batch(experiment_id: str, batch_id: str) -> pathlib.Path:
    """Resolve one explicitly named Pi 5 batch and validate its provenance."""
    if not batch_id or any(character not in "abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789._-" for character in batch_id):
        raise ValueError("batch_id contains unsafe characters")
    name = batch_id if batch_id.startswith("rpi5-") else f"rpi5-{batch_id}"
    path = _find_repo_root() / "eval" / "results" / experiment_id / name
    if not path.is_dir():
        raise FileNotFoundError(f"Canonical batch does not exist: {path}")
    validate_canonical_batch(path)
    return path


def validate_canonical_batch(path: pathlib.Path) -> str:
    """Reject mixed, dirty, throttled, incomplete, or non-Pi result leaves."""
    metadata_paths = sorted(path.rglob("metadata.json"))
    if not metadata_paths:
        raise ValueError(f"canonical batch has no metadata leaves: {path}")
    shas: set[str] = set()
    for metadata_path in metadata_paths:
        metadata = json.loads(metadata_path.read_text())
        if metadata.get("host_tag") != "rpi5":
            raise ValueError(f"non-rpi5 input: {metadata_path}")
        if metadata.get("git_dirty") is not False:
            raise ValueError(f"dirty canonical input: {metadata_path}")
        if not metadata.get("git_tags"):
            raise ValueError(f"untagged canonical input: {metadata_path}")
        if metadata.get("throttled") != "0x0":
            raise ValueError(f"throttled canonical input: {metadata_path}")
        status_path = metadata_path.parent / "canonical-status.json"
        if not status_path.is_file():
            raise ValueError(f"canonical input lacks completion receipt: {metadata_path.parent}")
        status = json.loads(status_path.read_text())
        if status.get("status") != "passed":
            raise ValueError(f"incomplete canonical input: {metadata_path.parent}")
        shas.add(str(metadata.get("git_sha", "")))
    if len(shas) != 1:
        raise ValueError(f"mixed source SHAs in canonical batch: {sorted(shas)}")
    return next(iter(shas))


def require_cross_architecture(pi_batch: pathlib.Path, x86_batch: pathlib.Path | None) -> None:
    """Refuse a cross-architecture conclusion until both native hosts exist."""
    validate_canonical_batch(pi_batch)
    if x86_batch is None or not x86_batch.is_dir():
        raise ValueError("E-Perf-5 is incomplete until matching x86 Linux data exists")
    validate_canonical_batch(x86_batch)


def _find_repo_root() -> pathlib.Path:
    """Walk up from this file until we find the eval/ directory."""
    p = pathlib.Path(__file__).resolve().parent
    while p != p.parent:
        if (p / "eval").is_dir():
            return p
        p = p.parent
    raise RuntimeError("Cannot locate repo root (no eval/ directory found)")
