"""Explicit result-batch resolution for WAFER evaluation notebooks."""

from __future__ import annotations

import hashlib
import json
import os
import pathlib
import re
from collections.abc import Mapping

from .results_layout import CANONICAL_ALIASES, ResultsLayout, resolve_alias_receipt

APPROVAL_RELATIVE_PATH = pathlib.Path(
    ".plans/rpi5-final-experiment-readiness/full-run-approval.json"
)
def resolve_result_batch(
    experiment_id: str,
    diagnostic_path: str | None = None,
    batch_id: str | None = None,
    results_root: pathlib.Path | str | None = None,
) -> pathlib.Path:
    """Resolve one explicit diagnostic path or approved canonical batch."""
    repo = _find_repo_root()
    layout = ResultsLayout.resolve(repo, results_root)
    if diagnostic_path is not None:
        path = pathlib.Path(diagnostic_path)
        if not path.is_absolute():
            path = layout.volume / path if layout.explicit else repo / path
        if layout.explicit:
            _reject_links_below(path, layout.raw)
        path = path.resolve()
        if layout.explicit and layout.raw.resolve() not in path.parents:
            raise ValueError(f"diagnostic input must be beneath the raw evidence root: {path}")
        if not path.is_dir():
            raise FileNotFoundError(f"Explicit diagnostic path does not exist: {path}")
        return path

    selected_batch = batch_id or os.environ.get("WAFER_EVAL_BATCH_ID")
    if not selected_batch:
        raise RuntimeError(
            "WAFER_EVAL_BATCH_ID or an explicit diagnostic_path is required"
        )
    return find_canonical_batch(experiment_id, selected_batch, results_root)


def resolve_analysis_batch(
    experiment_id: str,
    diagnostic_path: str | None = None,
    batch_id: str | None = None,
    results_root: pathlib.Path | str | None = None,
) -> tuple[pathlib.Path | None, bool]:
    """Resolve notebook input and identify whether canonical gates apply."""
    if diagnostic_path is not None:
        return resolve_result_batch(
            experiment_id,
            diagnostic_path=diagnostic_path,
            results_root=results_root,
        ), False
    selected_batch = batch_id or os.environ.get("WAFER_EVAL_BATCH_ID")
    if selected_batch:
        return find_canonical_batch(experiment_id, selected_batch, results_root), True
    return None, False


def find_canonical_batch(
    experiment_id: str,
    batch_id: str,
    results_root: pathlib.Path | str | None = None,
) -> pathlib.Path:
    """Resolve one explicitly named approved Pi 5 batch and validate it."""
    name = _canonical_batch_name(batch_id)
    repo = _find_repo_root()
    layout = ResultsLayout.resolve(repo, results_root)
    path = layout.raw_path(experiment_id, name)
    if path.is_dir():
        validate_canonical_batch(path, experiment_id, results_root)
        return path
    if experiment_id in CANONICAL_ALIASES:
        return _resolve_alias_batch(layout, experiment_id, name)
    raise FileNotFoundError(f"Canonical batch does not exist: {path}")


def find_canonical_ledger(
    batch_id: str, results_root: pathlib.Path | str | None = None
) -> pathlib.Path:
    """Resolve the ledger for one explicitly named canonical batch."""
    name = _canonical_batch_name(batch_id)
    layout = ResultsLayout.resolve(_find_repo_root(), results_root)
    path = layout.manifest_path("canonical-batches", name)
    if not path.is_dir():
        raise FileNotFoundError(f"Canonical batch ledger does not exist: {path}")
    return path


def resolve_analysis_output(
    kind: str,
    *parts: str,
    results_root: pathlib.Path | str | None = None,
) -> pathlib.Path:
    """Resolve a derived or report output without permitting raw writes."""
    return ResultsLayout.resolve(_find_repo_root(), results_root).analysis_path(
        kind, *parts
    )


def _resolve_alias_batch(
    layout: ResultsLayout, experiment_id: str, batch_name: str
) -> pathlib.Path:
    expected_source = CANONICAL_ALIASES.get(experiment_id)
    if expected_source is None:
        raise FileNotFoundError(
            f"Canonical batch does not exist and experiment is not an alias: {experiment_id}/{batch_name}"
        )
    receipt_root = layout.manifest_path("aliases", experiment_id, batch_name)
    receipts = sorted(receipt_root.rglob("run-*.json")) if receipt_root.is_dir() else []
    if not receipts:
        raise FileNotFoundError(
            f"Canonical batch or alias receipt does not exist: {experiment_id}/{batch_name}"
        )
    matrix = _read_object(_find_repo_root() / "eval/canonical-matrix.json", "canonical matrix")
    definitions = matrix.get("experiments")
    if not isinstance(definitions, dict) or not isinstance(
        definitions.get(experiment_id), dict
    ):
        raise ValueError(f"canonical matrix does not declare alias {experiment_id}")
    expected_units = _expected_units(definitions[experiment_id])
    observed_units: set[tuple[str, int]] = set()
    source_batches: set[pathlib.Path] = set()
    for receipt_path in receipts:
        receipt = _read_object(receipt_path, "alias receipt")
        if (
            receipt.get("schema_version") != 1
            or receipt.get("experiment") != experiment_id
            or receipt.get("shared_from_experiment") != expected_source
            or receipt.get("shared_measurement") is not True
        ):
            raise ValueError(f"malformed alias receipt: {receipt_path}")
        try:
            unit = (str(receipt["condition"]), int(receipt["run_index"]))
        except (KeyError, TypeError, ValueError) as error:
            raise ValueError(f"malformed alias receipt: {receipt_path}") from error
        if unit not in expected_units or unit in observed_units:
            raise ValueError(f"unexpected or duplicate alias receipt: {unit}")
        observed_units.add(unit)
        _, source = resolve_alias_receipt(receipt_path)
        source_batches.add(source.parents[1])
    if observed_units != expected_units:
        raise ValueError("alias receipt set is incomplete")
    if len(source_batches) != 1:
        raise ValueError("alias receipts do not resolve to one source batch")
    source_batch = next(iter(source_batches))
    validate_canonical_batch(
        source_batch,
        expected_source,
        layout.volume if layout.explicit else None,
    )
    return source_batch


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


def _reject_links_below(path: pathlib.Path, root: pathlib.Path) -> None:
    current = path.absolute()
    boundary = root.absolute()
    while True:
        if current.is_symlink():
            raise ValueError(f"evidence path must not use symlinks: {current}")
        if current == boundary:
            return
        if current == current.parent:
            raise ValueError(f"evidence path is outside the results root: {path}")
        current = current.parent


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
    path: pathlib.Path,
    experiment_id: str | None = None,
    results_root: pathlib.Path | str | None = None,
) -> str:
    """Reject unapproved, mixed, dirty, throttled, malformed, or incomplete input."""
    repo = _find_repo_root()
    layout = ResultsLayout.resolve(repo, results_root)
    if layout.explicit:
        _reject_links_below(path, layout.raw)
    resolved_path = path.resolve()
    if layout.explicit and layout.raw.resolve() not in resolved_path.parents:
        raise ValueError(f"canonical input must be beneath the raw evidence root: {path}")
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

    expected = _expected_units(definition)
    status_paths = sorted(path.rglob("canonical-status.json"))
    if not status_paths:
        raise ValueError(f"canonical batch has no completion receipts: {path}")
    attempts: dict[tuple[str, int], list[tuple[pathlib.Path, dict]]] = {}
    for status_path in status_paths:
        status = _read_object(status_path, "canonical status")
        leaf = status_path.parent
        relative = leaf.relative_to(path)
        match = re.fullmatch(r"run-(\d+)(?:-attempt-\d+)?", relative.name)
        if match is None or len(relative.parts) < 2:
            raise ValueError(f"malformed canonical status path: {status_path}")
        unit = ("/".join(relative.parts[:-1]), int(match.group(1)))
        if unit not in expected:
            raise ValueError(f"unexpected canonical attempt: {unit}")
        attempts.setdefault(unit, []).append((status_path, status))

    selected: list[pathlib.Path] = []
    for unit, unit_attempts in attempts.items():
        passed = [
            status_path.parent
            for status_path, status in unit_attempts
            if status.get("status") == "passed"
        ]
        if not passed:
            raise ValueError(f"failed canonical input: {unit}")
        if len(passed) != 1:
            raise ValueError(f"duplicate canonical run: {unit}")
        selected.extend(passed)

    shas: set[str] = set()
    observed: set[tuple[str, int]] = set()
    required_outputs = definition.get("required_outputs", [])
    if not isinstance(required_outputs, list):
        raise TypeError("canonical matrix required_outputs is malformed")

    for leaf in selected:
        metadata_path = leaf / "metadata.json"
        if not metadata_path.is_file():
            raise ValueError(f"canonical input lacks metadata: {leaf}")
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
