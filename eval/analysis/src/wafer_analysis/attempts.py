"""Attempt classes, the retry rule and admission, shared by the canonical runner and the analysis.

Every attempt of a scheduled unit (one experiment, condition and run index) ends in one class:

- ``passed``: the run met every check.
- ``sut_outcome``: the system under test failed a criterion the run measures. The attempt is
  admitted as data, counts against that criterion, and is never retried.
- ``infrastructure``: the host, the harness or the evidence failed. The attempt is not admitted
  and is retried in place up to the declared cap. An attempt directory without a receipt was
  interrupted and counts as one infrastructure attempt.

The runner and the result verifier load this module as a top-level module, so it uses no
package-relative imports.
"""

from __future__ import annotations

import csv
import json
import re
from collections.abc import Mapping
from dataclasses import dataclass
from pathlib import Path

PASSED = "passed"
SUT_OUTCOME = "sut_outcome"
INFRASTRUCTURE = "infrastructure"
ADMITTED = frozenset({PASSED, SUT_OUTCOME})

SUT_OUTCOME_REASONS = {
    "runtime-exit": "the system under test exited with a non-zero code after it started the "
    "pipeline, crashed, or had to be killed after the shutdown grace period",
    "containment-escape": "an attack was not stopped by its expected mechanism alone, or the "
    "runtime panicked",
    "message-loss": "messages were lost where the criterion expects none",
    "duplicates": "messages were delivered more than once where the criterion expects none",
    "swap-failed": "a hot-swap request did not succeed",
    "rollback-failed": "a failed replacement did not roll back",
    "memory-limit": "resident memory went over the declared limit",
}
INFRASTRUCTURE_REASONS = {
    "harness-error": "the host, a gate, the broker, the harness or a load generator failed "
    "before the run could be judged",
    "contract-violation": "the result failed a schema, provenance, telemetry or host check",
    "interrupted": "the attempt directory has no receipt",
}
INCOMPLETE_RUN_REASONS = frozenset({"runtime-exit", "swap-failed", "rollback-failed"})
STARTUP_REFUSALS = frozenset({1, 2})
OUTSIDE_INTERRUPTS = frozenset({130, 143})
RUNTIME_SYSTEMS = frozenset({"wafer", "native"})
SWAP_EXPERIMENTS = frozenset({"e-swap-1", "e-swap-4", "e-swap-independent-sessions"})
SWAP_REQUEST_EXPERIMENTS = SWAP_EXPERIMENTS | {"e-swap-3"}
ROLLBACK_EXPERIMENTS = frozenset({"e-swap-5", "e-swap-rollback-sessions"})
ZERO_LOSS_EXPERIMENTS = SWAP_EXPERIMENTS | ROLLBACK_EXPERIMENTS

_ATTEMPT_NAME = re.compile(r"run-(\d+)(?:-attempt-(\d+))?")


def runtime_exit_is_outcome(code: int) -> bool:
    """Whether a non-zero exit of the system under test is an outcome of that system.

    Exits 1 and 2 mean the runtime refused to start the pipeline (startup or configuration
    failure) and 130 and 143 mean a second interrupt reached it from outside the run; both are
    infrastructure failures. Any other non-zero code, a crash, or a kill after the shutdown
    grace period is an outcome. A negative code is Python's form of death by signal.
    """
    status = 128 - code if code < 0 else code
    return status != 0 and status not in STARTUP_REFUSALS | OUTSIDE_INTERRUPTS


def sut_outcome_reasons(leaf: Path, experiment: str) -> list[str]:
    """The criteria the system under test failed in one attempt, read from its artifacts.

    Artifacts that are absent or unreadable add no reason; the verifier judges them.
    """
    reasons = []
    metadata = _read_json(leaf / "metadata.json")
    if isinstance(metadata, dict) and metadata.get("system", "wafer") in RUNTIME_SYSTEMS:
        exit_codes = metadata.get("exit_codes")
        code = exit_codes.get("wafer_runtime") if isinstance(exit_codes, dict) else None
        if type(code) is int and runtime_exit_is_outcome(code):
            reasons.append("runtime-exit")
    containment = _read_json(leaf / "containment.json")
    if isinstance(containment, dict) and containment.get("contained") is False:
        reasons.append("containment-escape")
    requests = _read_json(leaf / "swap_requests.json")
    if isinstance(requests, list):
        if experiment in SWAP_REQUEST_EXPERIMENTS and any(
            not isinstance(request, dict) or request.get("http_status") != 200
            for request in requests
        ):
            reasons.append("swap-failed")
        if experiment in ROLLBACK_EXPERIMENTS and not all(
            isinstance(request, dict)
            and request.get("http_status") == 200
            and isinstance(request.get("body"), dict)
            and request["body"].get("status") == "rolled_back"
            for request in requests
        ):
            reasons.append("rollback-failed")
    sequence = None
    if experiment in ZERO_LOSS_EXPERIMENTS:
        sequence = _sequence_summary(leaf / "sequence.csv")
    backpressure = _read_json(leaf / "backpressure.json") if experiment == "e-backpressure" else None
    if isinstance(backpressure, dict):
        sequence = backpressure.get("sequence")
        counts = backpressure.get("counts")
        if (
            backpressure.get("policy") == "slow"
            and isinstance(counts, dict)
            and counts.get("attempted") != counts.get("delivered")
        ):
            reasons.append("message-loss")
        if backpressure.get("memory", {}).get("within_limit") is False:
            reasons.append("memory-limit")
    disruption = _read_json(leaf / "disruption-analysis.json") if experiment == "e-swap-3" else None
    if isinstance(disruption, dict) and disruption.get("strategy") == "wafer-hotswap":
        sequence = {"gaps": disruption.get("loss", 0), "duplicates": disruption.get("duplicates", 0)}
    if isinstance(sequence, dict):
        if experiment != "e-backpressure" and sequence.get("gaps", 0) > 0:
            reasons.append("message-loss")
        if sequence.get("duplicates", 0) > 0:
            reasons.append("duplicates")
    return reasons


def _read_json(path: Path) -> object:
    try:
        return json.loads(path.read_text())
    except (OSError, ValueError):
        return None


def _sequence_summary(path: Path) -> dict[str, int] | None:
    try:
        with path.open(newline="") as stream:
            rows = list(csv.DictReader(stream))
        if len(rows) != 1:
            return None
        return {
            "gaps": int(rows[0]["gap_msgs"]),
            "duplicates": int(rows[0]["duplicates_count"]),
        }
    except (KeyError, OSError, TypeError, ValueError):
        return None


def infrastructure_retries(matrix: Mapping, experiment: str) -> int:
    """The in-place retry cap the matrix declares for one experiment."""
    policy = matrix["final_campaign"]["attempt_policy"]
    if experiment in policy["gate_experiments"]:
        return 0
    return int(policy["infrastructure_retries"])


@dataclass(frozen=True)
class Attempt:
    path: Path
    run_index: int
    number: int
    outcome: str
    reasons: tuple[str, ...]


@dataclass(frozen=True)
class Unit:
    condition: str
    run_index: int
    attempts: tuple[Attempt, ...]
    state: str
    admitted: Attempt | None

    @property
    def retries(self) -> int:
        return max(len(self.attempts) - 1, 0)


def read_attempt(path: Path) -> Attempt:
    """Classify one attempt directory from its receipt; no receipt means it was interrupted."""
    match = _ATTEMPT_NAME.fullmatch(path.name)
    if match is None or path.is_symlink() or not path.is_dir():
        raise ValueError(f"malformed attempt path: {path}")
    receipt_path = path / "canonical-status.json"
    if receipt_path.is_symlink() or (
        receipt_path.is_file() and receipt_path.stat().st_nlink > 1
    ):
        raise ValueError(f"linked terminal receipt is forbidden: {receipt_path}")
    run_index = int(match.group(1))
    number = int(match.group(2) or 0)
    try:
        receipt = json.loads(receipt_path.read_text())
    except (OSError, ValueError):
        receipt = None
    if not isinstance(receipt, dict):
        return Attempt(path, run_index, number, INFRASTRUCTURE, ("interrupted",))
    status = receipt.get("status")
    if status == PASSED:
        return Attempt(path, run_index, number, PASSED, ())
    if status != "failed":
        raise ValueError(f"unknown attempt status {status!r}: {receipt_path}")
    reasons = receipt.get("reasons")
    if receipt.get("failure_class") == SUT_OUTCOME:
        if (
            not isinstance(reasons, list)
            or not reasons
            or not set(reasons) <= SUT_OUTCOME_REASONS.keys()
        ):
            raise ValueError(f"system outcome receipt names no known reason: {receipt_path}")
        return Attempt(path, run_index, number, SUT_OUTCOME, tuple(reasons))
    if not isinstance(reasons, list) or not reasons:
        reasons = ["harness-error"]
    return Attempt(path, run_index, number, INFRASTRUCTURE, tuple(str(r) for r in reasons))


def condition_attempts(condition_dir: Path, run_index: int) -> list[Attempt]:
    """Every attempt directory of one run index, oldest first, with or without a receipt."""
    if not condition_dir.is_dir():
        return []
    attempts = [
        read_attempt(path)
        for path in condition_dir.iterdir()
        if path.name == f"run-{run_index:02d}" or path.name.startswith(f"run-{run_index:02d}-")
    ]
    return sorted(attempts, key=lambda attempt: attempt.number)


def settle(attempts: list[Attempt], retries: int | None) -> tuple[str, Attempt | None]:
    """The state of one unit: ``admitted``, ``missing`` after its retries, or ``pending``.

    ``retries=None`` sets no cap, so a unit without an admitted attempt stays pending.
    """
    admitted = [attempt for attempt in attempts if attempt.outcome in ADMITTED]
    if len(admitted) > 1:
        raise ValueError(f"multiple admitted attempts: {[str(a.path) for a in admitted]}")
    if admitted:
        if admitted[0] is not attempts[-1]:
            raise ValueError(f"an attempt follows the admitted one: {admitted[0].path}")
        return "admitted", admitted[0]
    if retries is not None and len(attempts) > retries:
        return "missing", None
    return "pending", None


def batch_units(batch: Path, retries: int | None) -> list[Unit]:
    """Every unit with at least one attempt directory below a batch, settled under the cap."""
    grouped: dict[tuple[str, int], list[Attempt]] = {}
    for path in sorted(batch.rglob("run-*")):
        if _ATTEMPT_NAME.fullmatch(path.name) is None or not (path.is_dir() or path.is_symlink()):
            continue
        relative = path.relative_to(batch)
        if any(_ATTEMPT_NAME.fullmatch(part) for part in relative.parts[:-1]):
            continue
        attempt = read_attempt(path)
        condition = "/".join(relative.parts[:-1])
        grouped.setdefault((condition, attempt.run_index), []).append(attempt)
    units = []
    for (condition, run_index), attempts in sorted(grouped.items()):
        attempts.sort(key=lambda attempt: attempt.number)
        state, admitted = settle(attempts, retries)
        units.append(Unit(condition, run_index, tuple(attempts), state, admitted))
    return units
