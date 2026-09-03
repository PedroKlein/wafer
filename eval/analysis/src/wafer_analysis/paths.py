"""Explicit result-batch resolution for WAFER evaluation notebooks."""

from __future__ import annotations

import json
import os
import pathlib
from typing import Optional


def resolve_result_batch(
    experiment_id: str,
    diagnostic_path: Optional[str] = None,
    batch_id: Optional[str] = None,
) -> pathlib.Path:
    """Resolve one explicit diagnostic path or canonical batch."""
    repo = _find_repo_root()
    if diagnostic_path is not None:
        path = pathlib.Path(diagnostic_path)
        if not path.is_absolute():
            path = repo / path
        path = path.resolve()
        if not path.is_dir():
            raise FileNotFoundError(f"Explicit diagnostic path does not exist: {path}")
        return path

    selected_batch = batch_id or os.environ.get("WAFER_EVAL_BATCH_ID")
    if not selected_batch:
        raise RuntimeError(
            "WAFER_EVAL_BATCH_ID or an explicit diagnostic_path is required"
        )
    return find_canonical_batch(experiment_id, selected_batch)


def find_canonical_batch(experiment_id: str, batch_id: str) -> pathlib.Path:
    """Resolve one explicitly named Pi 5 batch and validate its provenance."""
    name = _canonical_batch_name(batch_id)
    path = _find_repo_root() / "eval" / "results" / experiment_id / name
    if not path.is_dir():
        raise FileNotFoundError(f"Canonical batch does not exist: {path}")
    validate_canonical_batch(path)
    return path


def find_canonical_ledger(batch_id: str) -> pathlib.Path:
    """Resolve the ledger for one explicitly named canonical batch."""
    name = _canonical_batch_name(batch_id)
    path = _find_repo_root() / "eval" / "results" / "canonical-batches" / name
    if not path.is_dir():
        raise FileNotFoundError(f"Canonical batch ledger does not exist: {path}")
    return path


def _canonical_batch_name(batch_id: str) -> str:
    if not batch_id or any(
        character
        not in "abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789._-"
        for character in batch_id
    ):
        raise ValueError("batch_id contains unsafe characters")
    return batch_id if batch_id.startswith("rpi5-") else f"rpi5-{batch_id}"


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
