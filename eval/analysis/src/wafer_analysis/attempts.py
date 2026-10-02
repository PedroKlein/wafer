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
    "pipeline, crashed, or had to be killed after the shutdown grace period; for eKuiper, the "
    "kuiper unit lost or replaced its main process during the run",
    "rule-error": "the eKuiper rule was not running at the end of the run, or its status "
    "carried an error message",
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
INCOMPLETE_RUN_REASONS = frozenset({"runtime-exit", "rule-error", "swap-failed", "rollback-failed"})
STARTUP_REFUSALS = frozenset({1, 2})
OUTSIDE_INTERRUPTS = frozenset({130, 143})
RUNTIME_SYSTEMS = frozenset({"wafer", "native"})
SWAP_EXPERIMENTS = frozenset({"e-swap-1", "e-swap-4", "e-swap-independent-sessions"})
SWAP_REQUEST_EXPERIMENTS = SWAP_EXPERIMENTS | {"e-swap-3"}
ROLLBACK_EXPERIMENTS = frozenset({"e-swap-5", "e-swap-rollback-sessions"})
ZERO_LOSS_EXPERIMENTS = SWAP_EXPERIMENTS | ROLLBACK_EXPERIMENTS
EKUIPER_UNIT_PROPERTIES = ("NRestarts", "ExecMainStatus", "MainPID")
EKUIPER_REPLACEMENT_RULE = "pipeline_a_v2"
# threshold-filter-v2 raises Pipeline A's lower bound from 50 to 60. Both E-Swap-3 eKuiper arms
# make the same change to the pipeline_a SQL the audit recorded before warm-up.
SWAP3_RULE_BOUND_CHANGE = ("temperature >= 50", "temperature >= 60")

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


def ekuiper_rule_running(status: object) -> bool:
    """Whether an eKuiper rule status reads ``running`` with an empty ``message``.

    eKuiper keeps a rule ``running`` while it restarts a topology that failed, and says so in
    ``message``; a manual start clears it.
    """
    return (
        isinstance(status, dict)
        and str(status.get("status", "")).lower() == "running"
        and not status.get("message")
    )


def ekuiper_unit_restarted(health: Mapping) -> bool:
    """Whether the kuiper unit lost or replaced its main process between the two snapshots."""
    before, after = (
        snapshot.get("service") if isinstance(snapshot, dict) else None
        for snapshot in (health.get("before"), health.get("after"))
    )
    return (
        isinstance(before, dict)
        and isinstance(after, dict)
        and any(before.get(key) != after.get(key) for key in ("NRestarts", "MainPID"))
    )


def ekuiper_health_reasons(health: Mapping) -> list[str]:
    """The outcomes the two snapshots in ``ekuiper-health.json`` show.

    ``before`` is taken once the rule runs, before warm-up, and ``after`` once the load
    generators are done, before the harness stops eKuiper. When the run replaced the rule,
    ``after`` holds the status of the rule named in ``replacement_rule``. A new restart count
    or main PID means the process that ran the measurement ended, a ``runtime-exit`` as a
    crash is for WAFER; the rule status after it belongs to the new process and is not judged.
    Otherwise a rule that does not run at the end or carries a message is a ``rule-error``. The
    per-operator exception counters are not judged: eKuiper counts each message it drops from
    a full buffer there, and the run measures that loss itself.
    """
    if not isinstance(health.get("before"), dict) or not isinstance(health.get("after"), dict):
        return []
    if ekuiper_unit_restarted(health):
        return ["runtime-exit"]
    if not ekuiper_rule_running(health["after"].get("rule_status")):
        return ["rule-error"]
    return []


def ekuiper_exit_code(health: Mapping) -> int | None:
    """The ``exit_codes.ekuiper`` that ``ekuiper-health.json`` shows for one run.

    ``0`` while the process that ran the measurement is still the unit's main process, its
    ``ExecMainStatus`` when the unit is down at the end, and ``None`` when systemd has already
    started a new main process, because starting one resets that status.
    """
    if not ekuiper_unit_restarted(health):
        return 0
    after = health["after"]["service"]
    return after["ExecMainStatus"] if after["MainPID"] == 0 else None


def swap_adopted(request: object) -> bool:
    """Whether one recorded hot-swap request shows its replacement adopted.

    The runtime answers 200 both when it adopted the replacement and when it rolled back to
    the old plugin, so only a body that says ``replacement_adopted`` and reports no rollback
    counts.
    """
    body = request.get("body") if isinstance(request, dict) else None
    return (
        isinstance(request, dict)
        and request.get("http_status") == 200
        and isinstance(body, dict)
        and body.get("replacement_adopted") is True
        and body.get("status") != "rolled_back"
    )


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
    health = _read_json(leaf / "ekuiper-health.json")
    if isinstance(health, dict):
        reasons.extend(ekuiper_health_reasons(health))
    containment = _read_json(leaf / "containment.json")
    if isinstance(containment, dict) and containment.get("contained") is False:
        reasons.append("containment-escape")
    requests = _read_json(leaf / "swap_requests.json")
    if isinstance(requests, list):
        if experiment == "e-swap-3" and not all(swap_adopted(request) for request in requests):
            reasons.append("swap-failed")
        elif experiment in SWAP_REQUEST_EXPERIMENTS and any(
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
