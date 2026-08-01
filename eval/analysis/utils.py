"""Path-discovery helpers for WAFER evaluation notebooks.

Centralises shakedown result-directory resolution so notebooks never
hardcode timestamps that rot as new runs land.
"""

from __future__ import annotations

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

    pattern = f"{host_tag}-*"
    parent = repo / "eval" / "results" / experiment_id
    candidates = sorted(parent.glob(pattern))
    if not candidates:
        raise FileNotFoundError(
            f"No shakedown directories matching {parent / pattern}"
        )
    return candidates[-1]


def _find_repo_root() -> pathlib.Path:
    """Walk up from this file until we find the eval/ directory."""
    p = pathlib.Path(__file__).resolve().parent
    while p != p.parent:
        if (p / "eval").is_dir():
            return p
        p = p.parent
    raise RuntimeError("Cannot locate repo root (no eval/ directory found)")
