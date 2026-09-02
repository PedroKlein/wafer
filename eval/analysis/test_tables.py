import json
from pathlib import Path

import pytest

from wafer_analysis.tables import hotswap_timeline_table


def test_hotswap_table_keeps_internal_and_sink_observations_side_by_side() -> None:
    evidence = {
        "measurement_source_leaf": "eval/results/e-swap-1/batch/steady/run-01-attempt-01",
        "events": [
            {
                "event_index": 0,
                "compile_ns": 100_000_000,
                "instantiate_ns": 10_000_000,
                "signal_ns": 1_000,
                "ack_ns": 2_000_000,
                "convergence_ns": 3_000_000,
                "http_total_ns": 120_000_000,
                "sink_observed_output_gap_ns": 0,
            },
            {
                "event_index": 1,
                "compile_ns": 100_000,
                "instantiate_ns": 200_000,
                "signal_ns": 1_000,
                "ack_ns": 300_000,
                "convergence_ns": 400_000,
                "http_total_ns": 2_000_000,
                "sink_observed_output_gap_ns": 1_000_000,
            },
        ],
    }

    table, explanation = hotswap_timeline_table(evidence)

    assert list(table.columns) == [
        "experiment",
        "condition",
        "event_index",
        "compile_ms",
        "instantiate_ms",
        "signal_ms",
        "ack_ms",
        "convergence_ms",
        "http_total_ms",
        "sink_observed_output_gap_ms",
        "measurement_source_leaf",
    ]
    assert table.loc[0, "http_total_ms"] == 120.0
    assert table.loc[0, "sink_observed_output_gap_ms"] == 0.0
    assert table.loc[1, "http_total_ms"] == 2.0
    assert table.loc[1, "sink_observed_output_gap_ms"] == 1.0
    assert "does not imply" in explanation
    assert "queued output can mask" in explanation


def test_hotswap_notebook_warns_against_queue_masking_and_deduplicates_sources() -> None:
    notebook = json.loads(
        (Path(__file__).parent / "notebooks/05-hotswap-timeline.ipynb").read_text()
    )
    source = "".join(
        "".join(cell.get("source", [])) for cell in notebook["cells"]
    )
    assert "queued output can mask internal disruption" in source.lower()
    assert "source in seen" in source
    assert "sink_gap_ms" in source
    assert "http_total_ms" in source
    assert "PENDING" in source


def test_hotswap_table_rejects_presentation_units_in_raw_evidence() -> None:
    evidence = {
        "measurement_source_leaf": "source",
        "events": [
            {
                "event_index": 0,
                "compile_ms": 1.0,
                "instantiate_ns": 1,
                "signal_ns": 1,
                "ack_ns": 1,
                "convergence_ns": 1,
                "http_total_ns": 1,
                "sink_observed_output_gap_ns": 1,
            }
        ],
    }

    with pytest.raises(ValueError, match="compile_ns"):
        hotswap_timeline_table(evidence)
