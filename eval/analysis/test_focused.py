import json
import pytest

from wafer_analysis.focused import (
    admitted_artifacts,
    admitted_runs,
    depth_run_records,
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


def write_target_load_leaf(
    batch,
    *,
    received_events: int,
    received_unique: int,
    duplicates: int,
    sequence_end: int | None = 60_000,
    sequence_csv: str = "event_type,seq_start,seq_end,count\n",
    condition: str = "wafer",
):
    leaf = batch / condition / "run-01-attempt-01"
    leaf.mkdir(parents=True)
    (leaf / "canonical-status.json").write_text('{"status":"passed"}')
    (leaf / "percentiles.json").write_text(
        json.dumps(
            {
                "total_count": received_events,
                "p50_ns": 1,
                "p95_ns": 2,
                "p99_ns": 3,
                "p999_ns": 4,
            }
        )
    )
    (leaf / "throughput.csv").write_text(
        "timestamp_ns,messages_received,throughput_msg_s,duration_ns\n"
        f"1,{received_events},999.983333,60000000000\n"
    )
    (leaf / "sequence.csv").write_text(sequence_csv)
    expected = 60_000 if sequence_end else received_unique
    (leaf / "subscriber-metadata.json").write_text(
        json.dumps(
            {
                "sequence_end_exclusive": sequence_end,
                "total_recorded": received_events,
                "sequence": {
                    "expected": expected,
                    "total_received": received_events,
                    "received_unique": received_unique,
                    "total_gaps": expected - received_unique,
                    "total_duplicates": duplicates,
                },
            }
        )
    )


def test_target_load_rows_reconcile_delivery_and_duplicates(tmp_path) -> None:
    write_target_load_leaf(
        tmp_path,
        received_events=60_000,
        received_unique=59_999,
        duplicates=1,
        sequence_csv="event_type,seq_start,seq_end,count\ngap,42,42,1\nduplicate,7,7,1\n",
    )

    row = target_load_rows(tmp_path).iloc[0]
    assert row["intended_messages"] == 60_000
    assert row["received_unique"] == 59_999
    assert row["loss_fraction"] == 1 / 60_000
    assert row["achieved_rate_msg_s"] == 59_999 / 60
    assert row["duplicates"] == 1


def test_target_load_rows_count_declared_tail_loss_beyond_the_kept_examples(tmp_path) -> None:
    write_target_load_leaf(
        tmp_path,
        received_events=58_000,
        received_unique=58_000,
        duplicates=0,
        sequence_csv="event_type,seq_start,seq_end,count\ngap,1,1,1\n",
    )

    row = target_load_rows(tmp_path).iloc[0]
    assert row["received_unique"] == 58_000
    assert row["loss_fraction"] == 2_000 / 60_000
    assert row["achieved_ratio"] == 58_000 / 60_000


def test_target_load_rows_reject_a_leaf_without_a_declared_range(tmp_path) -> None:
    write_target_load_leaf(
        tmp_path, received_events=59_999, received_unique=59_999, duplicates=0, sequence_end=None
    )

    with pytest.raises(ValueError, match="does not declare its measured sequence range"):
        target_load_rows(tmp_path)


def test_target_load_rows_reject_counter_mismatch(tmp_path) -> None:
    write_target_load_leaf(
        tmp_path, received_events=59_999, received_unique=59_998, duplicates=0
    )

    with pytest.raises(ValueError, match="counters do not reconcile"):
        target_load_rows(tmp_path)


def test_depth_run_records_report_the_loss_of_each_mqtt_depth_run(tmp_path) -> None:
    write_target_load_leaf(
        tmp_path, received_events=57_000, received_unique=57_000, duplicates=0, condition="depth-10"
    )

    assert depth_run_records(tmp_path, canonical=True) == [
        {
            "condition": "depth-10",
            "run_index": 1,
            "p50_ns": 1,
            "p95_ns": 2,
            "received_unique": 57_000,
            "loss_fraction": 3_000 / 60_000,
        }
    ]
    assert depth_run_records(tmp_path, canonical=False) == [
        {"condition": "depth-10", "run_index": 1, "p50_ns": 1, "p95_ns": 2}
    ]


def test_evidence_label_exposes_sample_units_and_claim_boundary() -> None:
    assert evidence_label(3, "nanoseconds", False) == (
        "N=3; units=nanoseconds; evidence=diagnostic; uncertainty=descriptive only"
    )


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
