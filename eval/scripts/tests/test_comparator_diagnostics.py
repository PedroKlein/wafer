#!/usr/bin/env python3

import csv
import importlib.util
import json
import sys
from pathlib import Path

import pytest

ROOT = Path(__file__).resolve().parents[3]
SCRIPT = ROOT / "eval/scripts/diagnose-mqtt-latency.py"
spec = importlib.util.spec_from_file_location("diagnose_mqtt_latency", SCRIPT)
assert spec and spec.loader
module = importlib.util.module_from_spec(spec)
sys.modules[spec.name] = module
spec.loader.exec_module(module)

DiagnosticError = module.DiagnosticError
analyze_traces = module.analyze_traces
profile_provenance = module.profile_provenance


def write_csv(path: Path, fieldnames: list[str], rows: list[dict[str, int]]) -> None:
    with path.open("w", newline="") as handle:
        writer = csv.DictWriter(handle, fieldnames=fieldnames)
        writer.writeheader()
        writer.writerows(rows)


def write_valid_trace(tmp_path: Path) -> tuple[Path, Path, Path]:
    published = tmp_path / "published.csv"
    received = tmp_path / "received.csv"
    metadata = tmp_path / "subscriber-metadata.json"
    write_csv(
        published,
        ["seq", "ts_ns"],
        [
            {"seq": 0, "ts_ns": 1_000_000},
            {"seq": 1, "ts_ns": 2_000_000},
            {"seq": 2, "ts_ns": 3_000_000},
        ],
    )
    write_csv(
        received,
        ["seq", "payload_ts_ns", "receive_ns", "latency_ns"],
        [
            {"seq": 0, "payload_ts_ns": 1_000_000, "receive_ns": 1_001_000, "latency_ns": 1_000},
            {"seq": 1, "payload_ts_ns": 2_000_000, "receive_ns": 2_003_000, "latency_ns": 3_000},
            {"seq": 2, "payload_ts_ns": 3_000_000, "receive_ns": 3_002_000, "latency_ns": 2_000},
        ],
    )
    metadata.write_text(
        json.dumps(
            {
                "total_messages": 3,
                "total_recorded": 3,
                "parse_errors": 0,
                "negative_latency_count": 0,
                "latency_p50_ns": 2_000,
                "latency_p95_ns": 3_000,
                "latency_p99_ns": 3_000,
                "sequence": {
                    "total_received": 3,
                    "total_gaps": 0,
                    "total_duplicates": 0,
                    "gap_ranges": [],
                    "duplicate_seqs": [],
                },
            }
        )
    )
    return published, received, metadata


def test_trace_analysis_reports_counts_percentiles_and_raw_pairs(tmp_path: Path) -> None:
    published, received, metadata = write_valid_trace(tmp_path)

    report = analyze_traces(published, received, metadata)

    assert report["offered_count"] == 3
    assert report["received_count"] == 3
    assert report["gaps"] == 0
    assert report["duplicates"] == 0
    assert report["latency_ns"] == {"p50": 2_000, "p95": 3_000, "p99": 3_000}
    assert report["raw_pairs"] == [
        {
            "published": {"seq": 0, "ts_ns": 1_000_000},
            "received": {"seq": 0, "payload_ts_ns": 1_000_000, "receive_ns": 1_001_000},
            "latency_ns": 1_000,
        },
        {
            "published": {"seq": 1, "ts_ns": 2_000_000},
            "received": {"seq": 1, "payload_ts_ns": 2_000_000, "receive_ns": 2_003_000},
            "latency_ns": 3_000,
        },
        {
            "published": {"seq": 2, "ts_ns": 3_000_000},
            "received": {"seq": 2, "payload_ts_ns": 3_000_000, "receive_ns": 3_002_000},
            "latency_ns": 2_000,
        },
    ]


@pytest.mark.parametrize(
    ("field", "value", "message"),
    [
        ("payload_ts_ns", 2_000_001, "timestamp changed"),
        ("seq", 20, "missing sequences"),
        ("receive_ns", 1_999_999, "negative latency"),
    ],
)
def test_trace_analysis_rejects_mutation_or_negative_latency(
    tmp_path: Path, field: str, value: int, message: str
) -> None:
    published, received, metadata = write_valid_trace(tmp_path)
    rows = list(csv.DictReader(received.open()))
    rows[1][field] = value
    write_csv(received, list(rows[0]), rows)

    with pytest.raises(DiagnosticError, match=message):
        analyze_traces(published, received, metadata)


def test_trace_analysis_rejects_missing_message(tmp_path: Path) -> None:
    published, received, metadata = write_valid_trace(tmp_path)
    rows = list(csv.DictReader(received.open()))[:-1]
    write_csv(received, list(rows[0]), rows)

    with pytest.raises(DiagnosticError, match="missing sequences: 2"):
        analyze_traces(published, received, metadata)


def test_profile_provenance_hashes_profile_and_checks_template(tmp_path: Path) -> None:
    profile = tmp_path / "profile.toml"
    profile.write_text(
        "[loadgen]\n"
        'topic = "diagnostic/input"\n'
        'payload_template = "telemetry-120b"\n'
        'payload_template_sha256 = "abc123"\n'
    )

    provenance = profile_provenance(profile, "abc123")

    assert provenance["path"] == str(profile)
    assert len(provenance["sha256"]) == 64
    assert provenance["payload_template"] == "telemetry-120b"
    assert provenance["payload_template_sha256"] == "abc123"

    with pytest.raises(DiagnosticError, match="template SHA-256 mismatch"):
        profile_provenance(profile, "different")


def test_canonical_profile_matches_renderer_fingerprint() -> None:
    fingerprint = "fffb14064f8d7956b5ce410c8298ab5c511bd46e5dffcde8389975337a0e38d1"

    provenance = profile_provenance(ROOT / "eval/loadgen/telemetry-120b.toml", fingerprint)

    assert provenance["payload_template"] == "telemetry-120b"
    assert provenance["payload_template_sha256"] == fingerprint


def test_artifact_metadata_is_diagnostic_only_and_has_no_sut_pid(tmp_path: Path) -> None:
    published, received, metadata = write_valid_trace(tmp_path)
    provenance = {
        "path": "profile.toml",
        "sha256": "a" * 64,
        "payload_template": "telemetry-120b",
        "payload_template_sha256": "b" * 64,
    }

    artifact = module.build_artifact(
        analyze_traces(published, received, metadata),
        provenance,
        {"checked_processes": ["wafer-runtime", "kuiperd"], "sut_pids": []},
    )

    assert artifact["system"] == "mqtt-loopback"
    assert artifact["thesis_evidence"] is False
    assert artifact["process_audit"]["sut_pids"] == []

    with pytest.raises(DiagnosticError, match="SUT process detected"):
        module.build_artifact(
            analyze_traces(published, received, metadata),
            provenance,
            {"checked_processes": ["wafer-runtime", "kuiperd"], "sut_pids": [{"pid": 42}]},
        )
