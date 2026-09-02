"""Focused-pilot presentation labels and artifact inventory."""

from __future__ import annotations

import pandas as pd


FOLLOWUP_ARTIFACTS = (
    ("eKuiper latency tail", "09-saturation.ipynb", "rate-sweep.json"),
    ("target load versus saturation", "09-saturation.ipynb", "rate-sweep-summary.json"),
    ("branch-A throughput and latency", "06-fault-injection.ipynb", "branch-isolation.json"),
    ("epoch recovery", "06-fault-injection.ipynb", "containment.json"),
    ("startup cache state", "10-aot-startup.ipynb", "startup.json"),
    ("bounded queue pressure", "09-backpressure.ipynb", "backpressure.json"),
    ("internal and sink-observed hot-swap timing", "05-hotswap-timeline.ipynb", "hotswap-analysis.json"),
)


def evidence_label(sample_count: int, units: str, thesis_evidence: bool) -> str:
    evidence = "thesis" if thesis_evidence else "diagnostic"
    uncertainty = "inferential" if thesis_evidence and sample_count >= 30 else "descriptive only"
    return f"N={sample_count}; units={units}; evidence={evidence}; uncertainty={uncertainty}"


def pending_record(question: str, reason: str, units: str) -> dict:
    return {
        "question": question,
        "status": "PENDING",
        "value": None,
        "units": units,
        "reason": reason,
        "thesis_evidence": False,
    }


def artifact_inventory(available: set[str] | None = None) -> pd.DataFrame:
    available = available or set()
    rows = []
    for question, notebook, artifact in FOLLOWUP_ARTIFACTS:
        rows.append(
            {
                "question": question,
                "notebook": notebook,
                "artifact": artifact,
                "status": "READY" if artifact in available else "PENDING",
                "thesis_evidence": False,
            }
        )
    return pd.DataFrame(rows)
