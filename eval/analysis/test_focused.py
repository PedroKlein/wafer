import json
from pathlib import Path

import pytest

from wafer_analysis.focused import (
    admitted_artifacts,
    admitted_runs,
    evidence_label,
    pending_record,
    percentile_rows,
    target_load_rows,
)

OUTCOME = {"status": "failed", "failure_class": "sut_outcome", "reasons": ["runtime-exit"]}


def test_admitted_artifacts_exclude_failed_and_incomplete_leaves(tmp_path) -> None:
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

    artifacts = admitted_artifacts(tmp_path, "percentiles.json")
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


def test_admitted_runs_keep_outcome_runs_that_stopped_early(tmp_path) -> None:
    retried = tmp_path / "panic" / "run-01-attempt-02"
    for attempt, receipt in (
        (tmp_path / "panic" / "run-01-attempt-01", {"status": "failed"}),
        (retried, {"status": "passed"}),
        (tmp_path / "panic" / "run-02-attempt-01", OUTCOME),
    ):
        attempt.mkdir(parents=True)
        (attempt / "canonical-status.json").write_text(json.dumps(receipt))
    (retried / "containment.json").write_text('{"contained": true}')

    records = admitted_runs(tmp_path, "containment.json")

    assert records == [
        {"condition": "panic", "run_index": 1, "contained": True, "sut_outcome_reasons": []},
        {"condition": "panic", "run_index": 2, "sut_outcome_reasons": ["runtime-exit"]},
    ]
    assert [path.parent.name for path, _ in admitted_artifacts(tmp_path, "containment.json")] == [
        "run-01-attempt-02"
    ]


def test_admitted_runs_reject_a_complete_run_without_its_artifact(tmp_path) -> None:
    leaf = tmp_path / "panic" / "run-01-attempt-01"
    leaf.mkdir(parents=True)
    (leaf / "canonical-status.json").write_text('{"status":"passed"}')

    with pytest.raises(ValueError, match="admitted attempt lacks containment.json"):
        admitted_runs(tmp_path, "containment.json")


def test_target_load_rows_reconcile_delivery_and_duplicates(tmp_path) -> None:
    leaf = tmp_path / "wafer" / "run-01-attempt-01"
    leaf.mkdir(parents=True)
    (leaf / "canonical-status.json").write_text('{"status":"passed"}')
    (leaf / "percentiles.json").write_text(
        json.dumps(
            {
                "total_count": 60_000,
                "p50_ns": 1,
                "p95_ns": 2,
                "p99_ns": 3,
                "p999_ns": 4,
            }
        )
    )
    (leaf / "throughput.csv").write_text(
        "timestamp_ns,messages_received,throughput_msg_s,duration_ns\n"
        "1,60000,999.983333,60000000000\n"
    )
    (leaf / "sequence.csv").write_text(
        "event_type,seq_start,seq_end,count\ngap,42,42,1\nduplicate,7,7,1\n"
    )

    row = target_load_rows(tmp_path).iloc[0]
    assert row["intended_messages"] == 60_000
    assert row["received_unique"] == 59_999
    assert row["loss_fraction"] == 1 / 60_000
    assert row["achieved_rate_msg_s"] == 59_999 / 60
    assert row["duplicates"] == 1


def test_target_load_rows_reject_counter_mismatch(tmp_path) -> None:
    leaf = tmp_path / "wafer" / "run-01-attempt-01"
    leaf.mkdir(parents=True)
    (leaf / "canonical-status.json").write_text('{"status":"passed"}')
    (leaf / "percentiles.json").write_text(
        json.dumps({"total_count": 59_999, "p50_ns": 1, "p95_ns": 2, "p99_ns": 3})
    )
    (leaf / "throughput.csv").write_text(
        "timestamp_ns,messages_received,throughput_msg_s,duration_ns\n"
        "1,59999,999.983333,60000000000\n"
    )
    (leaf / "sequence.csv").write_text("event_type,seq_start,seq_end,count\n")

    with pytest.raises(ValueError, match="gap total does not reconcile"):
        target_load_rows(tmp_path)


def test_evidence_label_exposes_sample_units_and_claim_boundary() -> None:
    assert evidence_label(3, "nanoseconds", False) == (
        "N=3; units=nanoseconds; evidence=diagnostic; uncertainty=descriptive only"
    )


def test_notebooks_label_evidence_and_pending_conditions() -> None:
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
    latency = json.loads((notebook_dir / "01-latency-cdf.ipynb").read_text())
    latency_source = "".join(
        "".join(cell.get("source", [])) for cell in latency["cells"]
    )
    assert "target_load_rows" in saturation_source
    assert "target_load_rows" in latency_source
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
