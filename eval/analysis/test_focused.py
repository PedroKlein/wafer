import json
from pathlib import Path

from wafer_analysis.focused import artifact_inventory, evidence_label, pending_record


def test_artifact_inventory_covers_every_followup_question() -> None:
    inventory = artifact_inventory({"backpressure.json", "startup.json"})
    assert set(inventory["question"]) == {
        "eKuiper latency tail",
        "target load versus saturation",
        "branch-A throughput and latency",
        "epoch recovery",
        "startup cache state",
        "bounded queue pressure",
        "internal and sink-observed hot-swap timing",
    }
    assert inventory.loc[inventory["artifact"] == "backpressure.json", "status"].item() == "READY"
    assert inventory.loc[inventory["artifact"] == "branch-isolation.json", "status"].item() == "PENDING"
    assert inventory["thesis_evidence"].eq(False).all()


def test_evidence_label_exposes_sample_units_and_claim_boundary() -> None:
    assert evidence_label(3, "nanoseconds", False) == (
        "N=3; units=nanoseconds; evidence=diagnostic; uncertainty=descriptive only"
    )


def test_focused_notebooks_label_evidence_and_pending_conditions() -> None:
    notebook_dir = Path(__file__).parent / "notebooks"
    for name in {
        "05-hotswap-timeline.ipynb",
        "06-fault-injection.ipynb",
        "09-saturation.ipynb",
        "09-backpressure.ipynb",
        "10-aot-startup.ipynb",
    }:
        notebook = json.loads((notebook_dir / name).read_text())
        source = "".join("".join(cell.get("source", [])) for cell in notebook["cells"])
        assert "N" in source, name
        assert "thesis_evidence=false" in source, name
        assert "descriptive-only uncertainty" in source, name
        assert "PENDING" in source, name
        assert "never zero" in source, name

    summary = json.loads((notebook_dir / "10-summary-stats.ipynb").read_text())
    summary_source = "".join(
        "".join(cell.get("source", [])) for cell in summary["cells"]
    )
    assert "PMIC internal-rail proxy" in summary_source
    assert "artifact_inventory()" in summary_source


def test_pending_record_uses_null_instead_of_zero() -> None:
    record = pending_record("epoch recovery", "no passed leaf", "nanoseconds")
    assert record == {
        "question": "epoch recovery",
        "status": "PENDING",
        "value": None,
        "units": "nanoseconds",
        "reason": "no passed leaf",
        "thesis_evidence": False,
    }
