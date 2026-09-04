import json
from pathlib import Path

from wafer_analysis.focused import (
    artifact_inventory,
    evidence_label,
    passed_artifacts,
    pending_record,
    percentile_rows,
)


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
    assert (
        inventory.loc[inventory["artifact"] == "backpressure.json", "status"].item()
        == "READY"
    )
    assert (
        inventory.loc[inventory["artifact"] == "branch-isolation.json", "status"].item()
        == "PENDING"
    )
    assert inventory["thesis_evidence"].eq(False).all()


def test_passed_artifacts_exclude_failed_and_incomplete_leaves(tmp_path) -> None:
    passed = tmp_path / "wafer" / "run-01"
    failed = tmp_path / "wafer" / "run-02"
    incomplete = tmp_path / "wafer" / "run-03"
    for path, status in ((passed, "passed"), (failed, "failed")):
        path.mkdir(parents=True)
        (path / "canonical-status.json").write_text(json.dumps({"status": status}))
        (path / "percentiles.json").write_text(
            json.dumps({"total_count": 10, "p50_ns": 1, "p95_ns": 2, "p99_ns": 3})
        )
    incomplete.mkdir(parents=True)
    (incomplete / "percentiles.json").write_text("{}")

    artifacts = passed_artifacts(tmp_path, "percentiles.json")
    assert [path.parent.name for path, _ in artifacts] == ["run-01"]
    rows = percentile_rows(tmp_path)
    assert rows.to_dict("records") == [
        {
            "condition": "wafer",
            "run": "run-01",
            "N_messages": 10,
            "p50_ns": 1,
            "p95_ns": 2,
            "p99_ns": 3,
            "p999_ns": None,
        }
    ]


def test_evidence_label_exposes_sample_units_and_claim_boundary() -> None:
    assert evidence_label(3, "nanoseconds", False) == (
        "N=3; units=nanoseconds; evidence=diagnostic; uncertainty=descriptive only"
    )


def test_focused_notebooks_label_evidence_and_pending_conditions() -> None:
    notebook_dir = Path(__file__).parent / "notebooks"
    for name in (
        "05-hotswap-timeline.ipynb",
        "06-fault-injection.ipynb",
        "09-saturation.ipynb",
        "09-backpressure.ipynb",
        "10-aot-startup.ipynb",
    ):
        notebook = json.loads((notebook_dir / name).read_text())
        source = "".join("".join(cell.get("source", [])) for cell in notebook["cells"])
        assert "N" in source, name
        assert "thesis_evidence" in source, name
        assert "descriptive" in source, name
        assert "PENDING" in source, name
        assert "never zero" in source, name

    saturation = json.loads((notebook_dir / "09-saturation.ipynb").read_text())
    saturation_source = "".join(
        "".join(cell.get("source", [])) for cell in saturation["cells"]
    )
    assert "set_yscale('log')" in saturation_source
    assert "SYSTEM_COLORS" in saturation_source
    assert "axhline(.01" in saturation_source
    assert "axhline(2.0" in saturation_source

    hotswap = json.loads((notebook_dir / "05-hotswap-timeline.ipynb").read_text())
    hotswap_source = "".join(
        "".join(cell.get("source", [])) for cell in hotswap["cells"]
    )
    assert "median_dip_percent" in hotswap_source
    assert "median_action_duration_ns" in hotswap_source
    assert "median_recovery_ns" in hotswap_source

    summary = json.loads((notebook_dir / "10-summary-stats.ipynb").read_text())
    summary_source = "".join(
        "".join(cell.get("source", [])) for cell in summary["cells"]
    )
    assert "PMIC internal-rail proxy" in summary_source
    assert "artifact_inventory(" in summary_source


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
