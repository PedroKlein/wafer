from __future__ import annotations

import hashlib
import json
from pathlib import Path

import pandas as pd
import pytest

from wafer_analysis.enhanced_visuals import (
    REQUIRED_FAMILIES,
    load_enhanced_visual_manifest,
    render_enhanced_visual_suite,
    validate_enhanced_visual_artifacts,
    validate_enhanced_visual_datasets,
    validate_enhanced_visual_manifest,
)
from wafer_analysis.results_layout import ResultsLayout

SHA = "a" * 40
TAG = "rpi5-eval-v5"
BATCH = "expanded-n5"


def layout(root: Path) -> ResultsLayout:
    volume = root / "eval"
    return ResultsLayout(
        volume=volume,
        raw=volume / "raw",
        manifests=volume / "manifests",
        derived=volume / "derived",
        reports=volume / "reports",
        explicit=True,
    )


def datasets() -> dict[str, pd.DataFrame]:
    manifest = load_enhanced_visual_manifest()
    definitions = {item["id"]: item for item in manifest["families"]}
    values: dict[str, pd.DataFrame] = {}
    for family_id in REQUIRED_FAMILIES:
        definition = definitions[family_id]
        rows = []
        for run_index in range(1, definition["independent_n"] + 1):
            rows.append(
                {
                    "experiment": definition["experiments"][0],
                    "condition": "condition-a",
                    "run_index": run_index,
                    "sample_id": f"run-{run_index:02d}",
                    "metric": "metric-a",
                    "value": run_index * 10.0,
                    "unit": definition["units"][0],
                    "x_value": float(run_index),
                    "source_git_sha": SHA,
                    "source_tag": TAG,
                    "source_dirty": False,
                    "batch_id": BATCH,
                    "evidence_class": definition["evidence_class"],
                    "thesis_evidence": False,
                    "n30_admitted": False,
                    "sample_unit": definition["sample_unit"],
                    "source_relative_path": (
                        f"manifests/candidate-batches/rpi5-{BATCH}/{family_id}.json"
                    ),
                    "source_sha256": "c" * 64,
                }
            )
        values[family_id] = pd.DataFrame(rows)

    for family_id in ("capacity-knee", "delivery-probability"):
        values[family_id]["system"] = "wafer"
        values[family_id]["offered_rate_msg_s"] = 8_000
        values[family_id]["classification"] = "good"
        values[family_id]["support_confounded"] = False
        values[family_id]["support_bad_from_rate_msg_s"] = 16_000
    values["delivery-probability"]["value"] = 1.0

    for family_id in (
        "interval-latency-throughput",
        "pmic-proxy-efficiency",
    ):
        base = values[family_id]
        intervals = []
        for interval_index in range(2):
            current = base.copy()
            current["interval_index"] = interval_index
            current["interval_start_ns"] = interval_index * 1_000_000_000
            current["interval_end_ns"] = (interval_index + 1) * 1_000_000_000
            current["expected_interval_rows"] = 2
            current["x_value"] = float(interval_index)
            current["sample_id"] = (
                current["sample_id"] + f"-interval-{interval_index:02d}"
            )
            intervals.append(current)
        values[family_id] = pd.concat(intervals, ignore_index=True)

    base = values["swap-10ms-timeline"]
    fine = []
    for bucket_index in range(400):
        current = base.copy()
        start = -2_000_000_000 + bucket_index * 10_000_000
        current["bucket_index"] = bucket_index
        current["bucket_start_ns"] = start
        current["bucket_end_ns"] = start + 10_000_000
        current["x_value"] = start / 1_000_000
        current["sample_id"] = current["sample_id"] + f"-bucket-{bucket_index:03d}"
        fine.append(current)
    values["swap-10ms-timeline"] = pd.concat(fine, ignore_index=True)
    values["swap-10ms-timeline"]["drain_right_censored"] = False

    for family_id in ("swap-hierarchy", "rollback-hierarchy"):
        base = values[family_id]
        first = base.assign(event_class="first-use-aot", event_count=1)
        first["sample_id"] = first["sample_id"] + "-first"
        cached = base.assign(event_class="cached", event_count=49)
        cached["sample_id"] = cached["sample_id"] + "-cached"
        values[family_id] = pd.concat([first, cached], ignore_index=True)
    return values


def materialize_sources(
    current: ResultsLayout, values: dict[str, pd.DataFrame]
) -> None:
    current.volume.mkdir(parents=True)
    for family_id, frame in values.items():
        path = current.volume / str(frame.iloc[0]["source_relative_path"])
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_text(json.dumps({"family": family_id}) + "\n")
        frame["source_sha256"] = hashlib.sha256(path.read_bytes()).hexdigest()


def hash_tree(root: Path) -> dict[str, str]:
    return {
        path.relative_to(root).as_posix(): hashlib.sha256(path.read_bytes()).hexdigest()
        for path in sorted(root.rglob("*"))
        if path.is_file()
    }


def test_manifest_declares_exact_twelve_families_and_explanations() -> None:
    manifest = load_enhanced_visual_manifest()
    assert tuple(manifest["required_families"]) == REQUIRED_FAMILIES
    assert tuple(item["id"] for item in manifest["families"]) == REQUIRED_FAMILIES
    assert len(manifest["families"]) == 12
    for item in manifest["families"]:
        assert item["source_fields"]
        assert item["units"]
        assert item["sample_unit"]
        assert item["how_to_read"]
        assert item["observed_n5_pattern"].startswith("PENDING")
        assert item["limits"]


def test_renderer_writes_only_derived_and_reports_with_deterministic_hashes(
    tmp_path: Path,
) -> None:
    first = layout(tmp_path / "first")
    values = datasets()
    materialize_sources(first, values)
    sentinel = first.raw / "sentinel"
    sentinel.parent.mkdir(parents=True)
    sentinel.write_text("immutable raw\n")
    before = hash_tree(first.raw)

    derived, reports = render_enhanced_visual_suite(
        values,
        first,
        batch_id=BATCH,
        expected_source_sha=SHA,
        expected_source_tag=TAG,
    )

    assert hash_tree(first.raw) == before
    assert derived == first.derived / "enhanced-n5" / BATCH
    assert reports == first.reports / "enhanced-n5" / BATCH
    assert len(list(derived.glob("*.csv"))) == 12
    assert len(list(derived.glob("*.svg"))) == 12
    validate_enhanced_visual_artifacts(derived, reports)

    second = layout(tmp_path / "second")
    second_values = datasets()
    materialize_sources(second, second_values)
    second_derived, _ = render_enhanced_visual_suite(
        second_values,
        second,
        batch_id=BATCH,
        expected_source_sha=SHA,
        expected_source_tag=TAG,
    )
    first_manifest = json.loads((derived / "artifact-manifest.json").read_text())
    second_manifest = json.loads(
        (second_derived / "artifact-manifest.json").read_text()
    )
    assert first_manifest["artifacts"] == second_manifest["artifacts"]


def test_dataset_validation_rejects_missing_n_wrong_source_or_tag_and_aliases() -> None:
    values = datasets()
    values["payload-knee"] = values["payload-knee"].iloc[:-1]
    with pytest.raises(ValueError, match="missing independent N"):
        validate_enhanced_visual_datasets(
            values,
            expected_source_sha=SHA,
            expected_source_tag=TAG,
            batch_id=BATCH,
        )

    values = datasets()
    values["payload-knee"].loc[0, "source_git_sha"] = "b" * 40
    with pytest.raises(ValueError, match="wrong source revision"):
        validate_enhanced_visual_datasets(
            values,
            expected_source_sha=SHA,
            expected_source_tag=TAG,
            batch_id=BATCH,
        )

    values = datasets()
    values["payload-knee"].loc[0, "source_tag"] = "wrong-tag"
    with pytest.raises(ValueError, match="wrong source tag"):
        validate_enhanced_visual_datasets(
            values,
            expected_source_sha=SHA,
            expected_source_tag=TAG,
            batch_id=BATCH,
        )

    values = datasets()
    values["swap-hierarchy"]["experiment"] = "e-swap-2"
    with pytest.raises(ValueError, match="alias counted as replication"):
        validate_enhanced_visual_datasets(
            values,
            expected_source_sha=SHA,
            expected_source_tag=TAG,
            batch_id=BATCH,
        )


def test_dataset_validation_rejects_censoring_drain_class_and_final_label_drift() -> None:
    values = datasets()
    values["capacity-knee"]["offered_rate_msg_s"] = 16_000
    with pytest.raises(ValueError, match="uncensored capacity row"):
        validate_enhanced_visual_datasets(
            values,
            expected_source_sha=SHA,
            expected_source_tag=TAG,
            batch_id=BATCH,
        )

    values = datasets()
    values["swap-10ms-timeline"]["drain_right_censored"] = True
    with pytest.raises(ValueError, match="right-censored drain"):
        validate_enhanced_visual_datasets(
            values,
            expected_source_sha=SHA,
            expected_source_tag=TAG,
            batch_id=BATCH,
        )

    values = datasets()
    values["interval-latency-throughput"] = values[
        "interval-latency-throughput"
    ].drop(index=0)
    with pytest.raises(ValueError, match="missing or excess intervals"):
        validate_enhanced_visual_datasets(
            values,
            expected_source_sha=SHA,
            expected_source_tag=TAG,
            batch_id=BATCH,
        )

    values = datasets()
    values["swap-10ms-timeline"] = values["swap-10ms-timeline"].loc[
        values["swap-10ms-timeline"]["bucket_index"] != 399
    ]
    with pytest.raises(ValueError, match="requires 400 fine buckets"):
        validate_enhanced_visual_datasets(
            values,
            expected_source_sha=SHA,
            expected_source_tag=TAG,
            batch_id=BATCH,
        )

    values = datasets()
    values["swap-hierarchy"].loc[
        values["swap-hierarchy"]["event_class"] == "cached", "event_count"
    ] = 48
    with pytest.raises(ValueError, match="mixes cold and cached events"):
        validate_enhanced_visual_datasets(
            values,
            expected_source_sha=SHA,
            expected_source_tag=TAG,
            batch_id=BATCH,
        )

    values = datasets()
    values["payload-knee"].loc[0, "value"] = float("inf")
    with pytest.raises(ValueError, match="finite numbers"):
        validate_enhanced_visual_datasets(
            values,
            expected_source_sha=SHA,
            expected_source_tag=TAG,
            batch_id=BATCH,
        )

    values = datasets()
    values["ekuiper-tail-association"]["thesis_evidence"] = True
    with pytest.raises(ValueError, match="labeled final"):
        validate_enhanced_visual_datasets(
            values,
            expected_source_sha=SHA,
            expected_source_tag=TAG,
            batch_id=BATCH,
        )


def test_source_artifact_hash_mismatch_is_rejected(tmp_path: Path) -> None:
    current = layout(tmp_path)
    values = datasets()
    materialize_sources(current, values)
    values["payload-knee"].loc[0, "source_sha256"] = "0" * 64
    with pytest.raises(ValueError, match="source artifact hash differs"):
        validate_enhanced_visual_datasets(
            values,
            expected_source_sha=SHA,
            expected_source_tag=TAG,
            batch_id=BATCH,
            layout=current,
        )


def test_renderer_rejects_raw_tree_output_and_repository_local_layout(
    tmp_path: Path,
) -> None:
    current = layout(tmp_path)
    current.volume.mkdir(parents=True)
    with pytest.raises(ValueError, match="derived or reports"):
        current.analysis_path("raw", "enhanced-n5", BATCH)

    local = ResultsLayout(
        volume=current.volume,
        raw=current.volume / "results",
        manifests=current.volume / "results",
        derived=current.derived,
        reports=current.reports,
        explicit=False,
    )
    with pytest.raises(ValueError, match="explicit results volume"):
        render_enhanced_visual_suite(
            datasets(),
            local,
            batch_id=BATCH,
            expected_source_sha=SHA,
            expected_source_tag=TAG,
        )


def test_manifest_and_artifact_validation_fail_closed(tmp_path: Path) -> None:
    manifest = load_enhanced_visual_manifest()
    changed = json.loads(json.dumps(manifest))
    changed["families"][0].pop("limits")
    with pytest.raises(ValueError, match="missing visual fields"):
        validate_enhanced_visual_manifest(changed)

    current = layout(tmp_path)
    values = datasets()
    materialize_sources(current, values)
    derived, reports = render_enhanced_visual_suite(
        values,
        current,
        batch_id=BATCH,
        expected_source_sha=SHA,
        expected_source_tag=TAG,
    )
    (derived / "payload-knee.csv").write_text("tampered\n")
    with pytest.raises(ValueError, match="csv hash differs"):
        validate_enhanced_visual_artifacts(derived, reports)
