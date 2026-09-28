"""Rejection rules for the latency evidence in BenchSink's measurement-window.json.

The canonical runner and the verifier both use them, so a run is judged the
same way in both places.
"""

from __future__ import annotations

# NTP clients slew small offsets and only step large ones (ntpd above 128 ms,
# systemd-timesyncd above 400 ms), so a real step is well above this, while
# a thread delayed between two clock reads stays well below it.
CLOCK_STEP_TOLERANCE_NS = 100_000_000


def latency_evidence_violations(window: dict) -> list[str]:
    """Clamped latency samples or a wall-clock step recorded by BenchSink.

    Windows written by the harness carry neither field and pass.
    """
    violations = [
        f"measurement-window.json latency_clamps.{name}.{kind} is {count}, must be zero"
        for name, clamps in window.get("latency_clamps", {}).items()
        for kind, count in clamps.items()
        if int(count) != 0
    ]
    step_ns = int(window.get("wall_clock_step_ns", 0))
    if abs(step_ns) > CLOCK_STEP_TOLERANCE_NS:
        violations.append(f"measurement-window.json wall clock stepped {step_ns} ns during the run")
    return violations
