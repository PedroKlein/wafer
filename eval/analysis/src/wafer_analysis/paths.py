"""Explicit result-batch resolution for WAFER evaluation notebooks."""

from __future__ import annotations

import hashlib
import json
import os
import pathlib
import re
from collections.abc import Mapping

APPROVAL_RELATIVE_PATH = pathlib.Path(
    ".plans/rpi5-final-experiment-readiness/full-run-approval.json"
)


def resolve_result_batch(
    experiment_id: str,
    diagnostic_path: str | None = None,
    batch_id: str | None = None,
) -> pathlib.Path:
    """Resolve one explicit diagnostic path or approved canonical batch."""
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


def resolve_analysis_batch(
    experiment_id: str,
    diagnostic_path: str | None = None,
    batch_id: str | None = None,
) -> tuple[pathlib.Path | None, bool]:
    """Resolve notebook input and identify whether canonical gates apply."""
    if diagnostic_path is not None:
        return resolve_result_batch(
            experiment_id, diagnostic_path=diagnostic_path
        ), False
    selected_batch = batch_id or os.environ.get("WAFER_EVAL_BATCH_ID")
    if selected_batch:
        return find_canonical_batch(experiment_id, selected_batch), True
    return None, False


def find_canonical_batch(experiment_id: str, batch_id: str) -> pathlib.Path:
    """Resolve one explicitly named approved Pi 5 batch and validate it."""
    name = _canonical_batch_name(batch_id)
    path = _find_repo_root() / "eval" / "results" / experiment_id / name
    if not path.is_dir():
        raise FileNotFoundError(f"Canonical batch does not exist: {path}")
    validate_canonical_batch(path, experiment_id)
    return path


def find_canonical_ledger(batch_id: str) -> pathlib.Path:
    """Resolve the ledger for one explicitly named canonical batch."""
    name = _canonical_batch_name(batch_id)
    path = _find_repo_root() / "eval" / "results" / "canonical-batches" / name
    if not path.is_dir():
        raise FileNotFoundError(f"Canonical batch ledger does not exist: {path}")
    return path


def analysis_evidence_status(
    path: pathlib.Path, *, canonical: bool
) -> dict[str, object]:
    """Return the immutable evidence label applied by analysis consumers."""
    if canonical:
        return {
            "mode": "canonical",
            "thesis_evidence": True,
            "uncertainty": "run-level inferential",
        }
    return {
        "mode": "diagnostic",
        "thesis_evidence": False,
        "uncertainty": "descriptive only",
    }


def _canonical_batch_name(batch_id: str) -> str:
    if not batch_id or any(
        character
        not in "abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789._-"
        for character in batch_id
    ):
        raise ValueError("batch_id contains unsafe characters")
    return batch_id if batch_id.startswith("rpi5-") else f"rpi5-{batch_id}"


def _read_object(path: pathlib.Path, label: str) -> dict:
    try:
        value = json.loads(path.read_text())
    except (OSError, ValueError) as error:
        raise ValueError(f"malformed {label}: {error}") from error
    if not isinstance(value, dict):
        raise TypeError(f"malformed {label}: expected an object")
    return value


def _approved_batch(repo: pathlib.Path) -> dict:
    approval_path = pathlib.Path(
        os.environ.get("WAFER_FULL_RUN_APPROVAL", repo / APPROVAL_RELATIVE_PATH)
    )
    try:
        approval = _read_object(approval_path, "full-run approval")
    except (TypeError, ValueError) as error:
        raise ValueError(f"unapproved canonical batch: {error}") from error
    matrix_path = repo / "eval/canonical-matrix.json"
    matrix_sha256 = hashlib.sha256(matrix_path.read_bytes()).hexdigest()
    if (
        approval.get("schema_version") != 1
        or approval.get("decision") != "APPROVE"
        or not isinstance(approval.get("batch_id"), str)
        or not isinstance(approval.get("wafer_git_sha"), str)
        or re.fullmatch(r"[0-9a-f]{40}", approval["wafer_git_sha"]) is None
        or approval.get("canonical_matrix_sha256") != matrix_sha256
    ):
        raise ValueError("unapproved canonical batch: approval identifiers are invalid")
    return approval


def _expected_units(definition: Mapping[str, object]) -> set[tuple[str, int]]:
    repetitions = int(definition.get("repetitions", 0))
    if repetitions <= 0:
        raise ValueError("canonical matrix declares an invalid repetition count")
    if "rate_points_msg_s" in definition:
        systems = definition.get("systems")
        rates = definition.get("rate_points_msg_s")
        if not isinstance(systems, list) or not isinstance(rates, list):
            raise ValueError("canonical matrix capacity design is malformed")
        return {
            (f"{system}/rate-{int(rate):05d}", run)
            for system in systems
            for rate in rates
            for run in range(1, repetitions + 1)
        }
    conditions = definition.get("conditions")
    if not isinstance(conditions, list):
        raise TypeError("canonical matrix experiment conditions are malformed")
    return {
        (str(condition), run)
        for condition in conditions
        for run in range(1, repetitions + 1)
    }


def _validate_json_artifact(path: pathlib.Path) -> None:
    value = _read_object(path, path.name)
    if path.name == "percentiles.json":
        required = {"total_count", "p50_ns", "p95_ns", "p99_ns", "p999_ns"}
        if not required <= value.keys() or any(
            not isinstance(value[field], (int, float)) for field in required
        ):
            raise ValueError(
                "malformed percentiles.json: missing numeric percentile fields"
            )
    elif (
        path.name
        in {
            "capacity-run.json",
            "disruption-analysis.json",
            "disruption-timeline.json",
            "burst-timeline.json",
            "hotswap-analysis.json",
            "throughput-buckets.json",
        }
        and value.get("schema_version") != 1
    ):
        raise ValueError(f"malformed {path.name}: schema_version must be 1")


def validate_canonical_batch(
    path: pathlib.Path, experiment_id: str | None = None
) -> str:
    """Reject unapproved, mixed, dirty, throttled, malformed, or incomplete input."""
    repo = _find_repo_root()
    experiment = experiment_id or path.parent.name
    approval = _approved_batch(repo)
    if path.name != _canonical_batch_name(approval["batch_id"]):
        raise ValueError(f"unapproved canonical batch: {path.name}")

    matrix = _read_object(repo / "eval/canonical-matrix.json", "canonical matrix")
    experiments = matrix.get("experiments")
    if not isinstance(experiments, dict) or experiment not in experiments:
        raise ValueError(f"canonical matrix does not declare experiment {experiment}")
    definition = experiments[experiment]
    if not isinstance(definition, dict):
        raise TypeError(f"canonical matrix experiment {experiment} is malformed")

    status_paths = sorted(path.rglob("canonical-status.json"))
    if not status_paths:
        raise ValueError(f"canonical batch has no completion receipts: {path}")
    for status_path in status_paths:
        status = _read_object(status_path, "canonical status")
        if status.get("status") != "passed":
            raise ValueError(f"failed canonical input: {status_path.parent}")
        if not (status_path.parent / "metadata.json").is_file():
            raise ValueError(f"canonical input lacks metadata: {status_path.parent}")

    metadata_paths = sorted(path.rglob("metadata.json"))
    if not metadata_paths:
        raise ValueError(f"canonical batch has no metadata leaves: {path}")
    shas: set[str] = set()
    observed: set[tuple[str, int]] = set()
    required_outputs = definition.get("required_outputs", [])
    if not isinstance(required_outputs, list):
        raise TypeError("canonical matrix required_outputs is malformed")

    for metadata_path in metadata_paths:
        leaf = metadata_path.parent
        metadata = _read_object(metadata_path, "metadata schema")
        required_metadata = {
            "experiment",
            "condition",
            "run_index",
            "host_tag",
            "git_sha",
            "git_dirty",
            "git_tags",
            "throttled",
        }
        if not required_metadata <= metadata.keys():
            raise ValueError(f"malformed metadata schema: {metadata_path}")
        if metadata.get("experiment") != experiment:
            raise ValueError(
                f"malformed metadata schema: wrong experiment in {metadata_path}"
            )
        if metadata.get("host_tag") != "rpi5":
            raise ValueError(f"non-rpi5 input: {metadata_path}")
        if metadata.get("git_dirty") is not False:
            raise ValueError(f"dirty canonical input: {metadata_path}")
        tags = metadata.get("git_tags")
        if not isinstance(tags, list) or not tags:
            raise ValueError(f"untagged canonical input: {metadata_path}")
        if metadata.get("throttled") != "0x0":
            raise ValueError(f"throttled canonical input: {metadata_path}")
        if metadata.get("thesis_evidence", True) is not True:
            raise ValueError(f"non-final canonical input: {metadata_path}")
        status_path = leaf / "canonical-status.json"
        if not status_path.is_file():
            raise ValueError(f"canonical input lacks completion receipt: {leaf}")
        sha = metadata.get("git_sha")
        if not isinstance(sha, str) or re.fullmatch(r"[0-9a-f]{40}", sha) is None:
            raise ValueError(
                f"malformed metadata schema: invalid git SHA in {metadata_path}"
            )
        shas.add(sha)
        try:
            unit = (str(metadata["condition"]), int(metadata["run_index"]))
        except (TypeError, ValueError) as error:
            raise ValueError(f"malformed metadata schema: {metadata_path}") from error
        if unit in observed:
            raise ValueError(f"duplicate canonical run: {unit}")
        observed.add(unit)
        for artifact in required_outputs:
            artifact_path = leaf / str(artifact)
            if not artifact_path.is_file():
                raise ValueError(
                    f"malformed canonical schema: missing {artifact} in {leaf}"
                )
            if artifact_path.stat().st_size == 0:
                raise ValueError(
                    f"malformed canonical schema: empty {artifact} in {leaf}"
                )
            if artifact_path.suffix == ".json":
                _validate_json_artifact(artifact_path)

    if len(shas) != 1:
        raise ValueError(f"mixed source SHAs in canonical batch: {sorted(shas)}")
    source_sha = next(iter(shas))
    if source_sha != approval["wafer_git_sha"]:
        raise ValueError("canonical input SHA differs from approval")
    expected = _expected_units(definition)
    if observed != expected:
        missing = sorted(expected - observed)
        extra = sorted(observed - expected)
        raise ValueError(
            "incomplete canonical batch: "
            f"expected {len(expected)} runs, observed {len(observed)}, "
            f"missing={missing[:3]}, extra={extra[:3]}"
        )
    return source_sha


def require_cross_architecture(
    pi_batch: pathlib.Path, x86_batch: pathlib.Path | None
) -> None:
    """Refuse a cross-architecture conclusion until both native hosts exist."""
    if x86_batch is None or not x86_batch.is_dir():
        raise ValueError("E-Perf-5 is incomplete until matching x86 Linux data exists")
    validate_canonical_batch(pi_batch, "e-perf-5")
    validate_canonical_batch(x86_batch, "e-perf-5")


def _find_repo_root() -> pathlib.Path:
    """Walk up from this file until we find the eval/ directory."""
    p = pathlib.Path(__file__).resolve().parent
    while p != p.parent:
        if (p / "eval").is_dir():
            return p
        p = p.parent
    raise RuntimeError("Cannot locate repo root (no eval/ directory found)")
