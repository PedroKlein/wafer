from __future__ import annotations

import hashlib
import importlib.util
import json
from pathlib import Path

import pandas as pd
import pytest

from wafer_analysis.canonical import ekuiper_profile_tables
from wafer_analysis.enhanced_visuals import REQUIRED_FAMILIES, validate_enhanced_visual_artifacts
from wafer_analysis.expanded_n5 import (
    ALLOWED_CONTROL_GENERATIONS,
    ALLOWED_EVIDENCE_RELEASES,
    ExpandedN5Selection,
    PRODUCTION_COUNTS,
    SelectedEvidence,
    build_expanded_n5_datasets,
    classify_release_pairs,
    completed_visual_manifest,
    load_expanded_n5_selection,
    render_expanded_n5_results,
    validate_expanded_n5_datasets,
)
from wafer_analysis.results_layout import ResultsLayout


def write_json(path: Path, value: object) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(json.dumps(value, indent=2, sort_keys=True) + "\n")


def layout(tmp_path: Path) -> ResultsLayout:
    volume = tmp_path / "volume"
    for name in ("raw", "manifests", "derived", "reports"):
        (volume / name).mkdir(parents=True, exist_ok=True)
    return ResultsLayout(
        volume=volume,
        raw=volume / "raw",
        manifests=volume / "manifests",
        derived=volume / "derived",
        reports=volume / "reports",
        explicit=True,
    )


def selection_fixture(tmp_path: Path) -> tuple[ResultsLayout, Path, Path, Path, Path]:
    current = layout(tmp_path)
    batch = "expanded-n5-fixture"
    selected: dict[str, dict] = {}
    for run_index, (tag, sha) in enumerate(ALLOWED_EVIDENCE_RELEASES.items(), 1):
        key = f"e-compare-ekuiper-profile/rate-01000/profiled/run-{run_index:02d}"
        leaf = current.raw / f"e-compare-ekuiper-profile/rpi5-{batch}/rate-01000/profiled/run-{run_index:02d}-attempt-01"
        metadata = {
            "experiment": "e-compare-ekuiper-profile",
            "condition": "rate-01000/profiled",
            "run_index": run_index,
            "git_sha": sha,
            "git_tags": [tag],
            "git_dirty": False,
            "campaign_started": False,
            "thesis_evidence": False,
            "n30_admitted": False,
            "expanded_n5_rehearsal": {
                "batch_id": batch,
                "release_tag": tag,
                "wafer_git_sha": sha,
                "control_generation": "continuation-09",
                "pool_with_final_campaign": False,
                "thesis_evidence": False,
            },
        }
        write_json(leaf / "metadata.json", metadata)
        write_json(leaf / "canonical-status.json", {"status": "passed"})
        write_json(leaf / "ekuiper-runtime-summary.json", {"p99_ns": run_index})
        selected[key] = {
            "path": leaf.relative_to(current.volume).as_posix(),
            "metadata_sha256": hashlib.sha256((leaf / "metadata.json").read_bytes()).hexdigest(),
            "status_sha256": hashlib.sha256((leaf / "canonical-status.json").read_bytes()).hexdigest(),
            "release_tag": tag,
            "wafer_git_sha": sha,
            "control_generation": "continuation-09",
            "control_batch": "B09-ekuiper-profile",
        }
    manifest = current.manifests / "expanded-n5.sha256"
    lines = []
    for path in sorted(current.raw.rglob("*")):
        if path.is_file():
            lines.append(
                f"{hashlib.sha256(path.read_bytes()).hexdigest()}  {path.relative_to(current.volume).as_posix()}\n"
            )
    manifest.write_text("".join(lines))
    composite = current.manifests / f"n5-batches/rpi5-{batch}/composite-index.json"
    write_json(
        composite,
        {
            "schema_version": 1,
            "classification": "diagnostic-expanded-n5-composite",
            "batch_id": batch,
            "campaign_started": False,
            "thesis_evidence": False,
            "n30_admitted": False,
            "selection_rule": "exact terminal reconciliation selection; no attempt pooling",
            "raw_manifest_sha256": hashlib.sha256(manifest.read_bytes()).hexdigest(),
            "terminal_reconciliation_sha256": "a" * 64,
            "schedule_sha256": "b" * 64,
            "supervisor_terminal_sha256": "c" * 64,
            "counts": {
                "schedule_records": 4,
                "selected_raw_leaves": 4,
                "aliases": 0,
                "qualified_prerequisites": 0,
                "unselected_attempts": 0,
                "unselected_failed_attempts": 0,
            },
            "release_composition": {tag: 1 for tag in ALLOWED_EVIDENCE_RELEASES},
            "control_composition": {"continuation-09": 4},
            "selected": selected,
            "aliases": {},
            "qualified_prerequisites": [],
            "unselected_attempts": [],
        },
    )
    source_seal = composite.with_name("source-seal.json")
    write_json(
        source_seal,
        {
            "state": "source-host-sealed",
            "batch_id": batch,
            "manifest_sha256": hashlib.sha256(manifest.read_bytes()).hexdigest(),
            "composite_sha256": hashlib.sha256(composite.read_bytes()).hexdigest(),
        },
    )
    handoff = current.manifests / "storage-qualification/fixture/handoff-macos.json"
    write_json(
        handoff,
        {
            "state": "handoff-verified",
            "host": "macos",
            "batch_id": batch,
            "manifest_sha256": hashlib.sha256(manifest.read_bytes()).hexdigest(),
            "source_seal_sha256": hashlib.sha256(source_seal.read_bytes()).hexdigest(),
            "composite_sha256": hashlib.sha256(composite.read_bytes()).hexdigest(),
        },
    )
    return current, manifest, composite, source_seal, handoff


def rebind_fixture(
    current: ResultsLayout,
    manifest: Path,
    composite: Path,
    source_seal: Path,
    handoff: Path,
) -> None:
    manifest.write_text(
        "".join(
            f"{hashlib.sha256(path.read_bytes()).hexdigest()}  "
            f"{path.relative_to(current.volume).as_posix()}\n"
            for path in sorted(current.raw.rglob("*"))
            if path.is_file()
        )
    )
    composite_value = json.loads(composite.read_text())
    composite_value["raw_manifest_sha256"] = hashlib.sha256(manifest.read_bytes()).hexdigest()
    write_json(composite, composite_value)
    source_value = json.loads(source_seal.read_text())
    source_value["manifest_sha256"] = hashlib.sha256(manifest.read_bytes()).hexdigest()
    source_value["composite_sha256"] = hashlib.sha256(composite.read_bytes()).hexdigest()
    write_json(source_seal, source_value)
    handoff_value = json.loads(handoff.read_text())
    handoff_value["manifest_sha256"] = hashlib.sha256(manifest.read_bytes()).hexdigest()
    handoff_value["composite_sha256"] = hashlib.sha256(composite.read_bytes()).hexdigest()
    handoff_value["source_seal_sha256"] = hashlib.sha256(source_seal.read_bytes()).hexdigest()
    write_json(handoff, handoff_value)


def test_production_constants_match_sealing_tool() -> None:
    script = Path(__file__).resolve().parents[2] / "eval/scripts/verify-storage-receipt.py"
    spec = importlib.util.spec_from_file_location("verify_storage_receipt", script)
    assert spec is not None and spec.loader is not None
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)

    assert ALLOWED_EVIDENCE_RELEASES == module.EXPANDED_N5_RELEASES
    assert ALLOWED_CONTROL_GENERATIONS == module.EXPANDED_N5_CONTROLS
    assert PRODUCTION_COUNTS == module.EXPANDED_N5_COMPOSITION


def test_selection_accepts_exact_mixed_v10_v13_composite(tmp_path: Path) -> None:
    current, manifest, composite, source_seal, handoff = selection_fixture(tmp_path)

    evidence = load_expanded_n5_selection(
        current,
        composite,
        source_seal,
        manifest,
        handoff,
        expected_counts={
            "schedule_records": 4,
            "selected_raw_leaves": 4,
            "aliases": 0,
            "qualified_prerequisites": 0,
            "unselected_attempts": 0,
            "unselected_failed_attempts": 0,
        },
    )

    assert set(evidence.releases) == set(ALLOWED_EVIDENCE_RELEASES)
    assert len(evidence.selected) == 4


@pytest.mark.parametrize("mutation", ["unknown", "dirty", "unselected", "relabeled", "v14"])
def test_selection_rejects_unapproved_or_unselected_evidence(
    tmp_path: Path, mutation: str
) -> None:
    current, manifest, composite, source_seal, handoff = selection_fixture(tmp_path)
    value = json.loads(composite.read_text())
    key, selected = next(iter(value["selected"].items()))
    metadata_path = current.volume / selected["path"] / "metadata.json"
    metadata = json.loads(metadata_path.read_text())
    if mutation == "unknown":
        metadata["git_sha"] = "f" * 40
    elif mutation == "dirty":
        metadata["git_dirty"] = True
    elif mutation == "unselected":
        value["selected"].pop(key)
        value["counts"]["selected_raw_leaves"] = 3
        value["counts"]["schedule_records"] = 3
    elif mutation == "relabeled":
        metadata["git_tags"] = ["rpi5-final-rc-v11"]
    else:
        metadata["git_sha"] = "d" * 40
        metadata["git_tags"] = ["rpi5-final-rc-v15"]
    if mutation != "unselected":
        write_json(metadata_path, metadata)
        selected["metadata_sha256"] = hashlib.sha256(metadata_path.read_bytes()).hexdigest()
    write_json(composite, value)
    rebind_fixture(current, manifest, composite, source_seal, handoff)

    with pytest.raises(ValueError):
        load_expanded_n5_selection(
            current,
            composite,
            source_seal,
            manifest,
            handoff,
            expected_counts={
                "schedule_records": 4,
                "selected_raw_leaves": 4,
                "aliases": 0,
                "qualified_prerequisites": 0,
                "unselected_attempts": 0,
                "unselected_failed_attempts": 0,
            },
        )


def analysis_selection_fixture(tmp_path: Path) -> ExpandedN5Selection:
    current = layout(tmp_path)
    selected: dict[str, SelectedEvidence] = {}

    def add_leaf(
        experiment: str,
        condition: str,
        run_index: int,
        artifacts: dict[str, str],
        *,
        tag: str = "rpi5-final-rc-v13",
        control: str = "continuation-10",
    ) -> None:
        key = f"{experiment}/{condition}/run-{run_index:02d}"
        leaf = current.raw / experiment / "rpi5-expanded-n5-fixture" / condition / f"run-{run_index:02d}-attempt-01"
        for name, content in artifacts.items():
            path = leaf / name
            path.parent.mkdir(parents=True, exist_ok=True)
            path.write_text(content)
        write_json(leaf / "canonical-status.json", {"status": "passed"})
        write_json(leaf / "metadata.json", {"git_dirty": False})
        selected[key] = SelectedEvidence(
            result_key=key,
            path=leaf.relative_to(current.volume).as_posix(),
            release_tag=tag,
            wafer_git_sha=ALLOWED_EVIDENCE_RELEASES[tag],
            control_generation=control,
            control_batch="fixture",
            metadata_sha256=hashlib.sha256((leaf / "metadata.json").read_bytes()).hexdigest(),
            status_sha256=hashlib.sha256((leaf / "canonical-status.json").read_bytes()).hexdigest(),
        )

    interval = {
        "row_count": 2,
        "rows": [
            {
                "interval_start_ns": index * 1_000_000_000,
                "interval_end_ns": (index + 1) * 1_000_000_000,
                "latency_p99_ns": {"status": "available", "value": 100_000 + index},
                "pmic_internal_rail_proxy_watts": {"status": "available", "value": 2.0},
                "throughput_messages_per_second": 1_000.0,
            }
            for index in range(2)
        ],
    }
    for condition in ("wafer", "native", "ekuiper"):
        for run_index in range(1, 6):
            add_leaf(
                "e-perf-1",
                condition,
                run_index,
                {"interval-metrics.json": json.dumps(interval)},
                tag="rpi5-final-rc-v10",
                control="legacy-v10",
            )

    grids = {
        "mqtt-loopback": [*range(4_000, 16_000, 1_000), 15_250, 15_500, 15_750, 16_000],
        "native": list(range(8_000, 16_000, 1_000)),
        "wafer": list(range(8_000, 16_000, 1_000)),
        "ekuiper": list(range(4_000, 9_000, 1_000)),
    }
    for system, rates in grids.items():
        for rate in rates:
            for run_index in range(1, 6):
                bad_support = system == "mqtt-loopback" and rate >= 8_000
                intended = rate * 60
                undelivered = intended // 50 if bad_support else 0
                capacity = {
                    "system": system,
                    "rate_msg_s": rate,
                    "run_index": run_index,
                    "messages": {
                        "intended": intended,
                        "total_undelivered": undelivered,
                        "duplicates": 0,
                    },
                    "rates_msg_s": {
                        "achieved": rate * (0.98 if bad_support else 1.0),
                        "achieved_ratio": 0.98 if bad_support else 1.0,
                    },
                }
                add_leaf(
                    "e-perf-capacity-knee",
                    f"{system}/rate-{rate:05d}",
                    run_index,
                    {"capacity-run.json": json.dumps(capacity)},
                )

    payloads = (
        ("120b", 120),
        ("1kb", 1_024),
        ("8kb", 8_192),
        ("10kb", 10_240),
        ("16kb", 16_384),
        ("32kb", 32_768),
        ("64kb", 65_536),
        ("100kb", 102_400),
        ("128kb", 131_072),
        ("256kb", 262_144),
    )
    for condition, size in payloads:
        for run_index in range(1, 6):
            add_leaf(
                "e-perf-payload-refinement",
                condition,
                run_index,
                {
                    "payload-manifest.json": json.dumps(
                        {"payload_bytes": size, "payload_sha256": hashlib.sha256(b"B" * size).hexdigest()}
                    ),
                    "percentiles.json": json.dumps(
                        {"p50_ns": size, "p95_ns": size * 2, "p99_ns": size * 3}
                    ),
                },
            )
    for depth in (1, 3, 5, 10, 20, 50):
        for run_index in range(1, 6):
            add_leaf(
                "e-perf-depth-extension",
                f"depth-{depth}",
                run_index,
                {
                    "topology-manifest.json": json.dumps({"depth": depth}),
                    "percentiles.json": json.dumps(
                        {"p50_ns": depth * 10, "p95_ns": depth * 20, "p99_ns": depth * 30}
                    ),
                    "memory.csv": "elapsed_ms,rss_bytes\n0,1024\n1,2048\n",
                },
                tag="rpi5-final-rc-v10",
                control="legacy-v10",
            )

    buckets = [
        {
            "start_offset_ns": -2_000_000_000 + index * 10_000_000,
            "end_offset_ns": -2_000_000_000 + (index + 1) * 10_000_000,
            "rate_msg_s": 1_000.0,
        }
        for index in range(400)
    ]
    for experiment, conditions in (
        ("e-swap-3", ("wafer-hotswap", "wafer-restart", "ekuiper-restart")),
        ("e-swap-4", ("burst-2x",)),
    ):
        for condition in conditions:
            for run_index in range(1, 6):
                artifacts = {"throughput-buckets-10ms.json": json.dumps({"buckets": buckets})}
                if experiment == "e-swap-4":
                    artifacts["burst-timeline.json"] = json.dumps(
                        {"drain_right_censored": False}
                    )
                add_leaf(
                    experiment,
                    condition,
                    run_index,
                    artifacts,
                    tag="rpi5-final-rc-v12",
                    control="continuation-08",
                )

    for experiment, condition, filename in (
        ("e-swap-independent-sessions", "steady", "hotswap-analysis.json"),
        ("e-swap-rollback-sessions", "process-trap-rollback", "rollback.json"),
    ):
        for run_index in range(1, 6):
            events = []
            for event_index in range(50):
                event = {
                    "event_index": event_index,
                    "event_class": "first-use-aot" if event_index == 0 else "cached",
                }
                if experiment == "e-swap-independent-sessions":
                    event["sink_observed_output_gap_ns"] = 100 + event_index
                else:
                    event["rollback_ns"] = 100 + event_index
                events.append(event)
            add_leaf(experiment, condition, run_index, {filename: json.dumps({"events": events})})

    for rate in (1_000, 4_000, 8_000):
        for run_index in range(1, 6):
            tag = "rpi5-final-rc-v12" if run_index == 1 and rate < 8_000 else "rpi5-final-rc-v13"
            control = "continuation-08" if tag == "rpi5-final-rc-v12" else "continuation-09"
            for arm in ("profiled", "unprofiled-control"):
                summary = {
                    "rate_msg_s": rate,
                    "run_index": run_index,
                    "profiler_state": arm,
                    "latency_ns": {"p95": 200_000, "p99": 300_000},
                }
                add_leaf(
                    "e-compare-ekuiper-profile",
                    f"rate-{rate:05d}/{arm}",
                    run_index,
                    {"ekuiper-runtime-summary.json": json.dumps(summary)},
                    tag=tag,
                    control=control,
                )

    host = current.raw / "e-host-thermal-storage/host-characterization-fixture"
    host.mkdir(parents=True)
    (host / "host-telemetry.csv").write_text(
        "timestamp_utc,monotonic_ns,phase,phase_elapsed_seconds,boot_id,temperature_millicelsius,cpu_frequency_hz,throttled,pmic_internal_rail_proxy_watts,memory_available_bytes,memory_psi_some_avg10,usb_read_bytes_per_second,usb_write_bytes_per_second\n"
        "t,1,idle,0.0,boot,25000,2400000000,0x0,2.0,1,0.0,0.0,0.0\n"
        "t,2,usb-read,0.0,boot,26000,2400000000,0x0,2.1,1,0.0,100.0,0.0\n"
    )
    write_json(
        host / "usb-integrity.json",
        {
            "checks": [
                {
                    "phase": "usb-write",
                    "bytes": 1000,
                    "elapsed_seconds": 2.0,
                    "expected_sha256": "a" * 64,
                    "observed_sha256": "a" * 64,
                },
                {
                    "phase": "usb-read",
                    "bytes": 2000,
                    "elapsed_seconds": 2.0,
                    "expected_sha256": "b" * 64,
                    "observed_sha256": "b" * 64,
                },
            ],
            "checksum_mismatch_count": 0,
        },
    )
    (host / "kernel-io.log").write_text("")
    write_json(host / "host-load-ladder.json", {"status": "passed"})

    manifest = {
        path.relative_to(current.volume).as_posix(): hashlib.sha256(path.read_bytes()).hexdigest()
        for path in sorted(current.raw.rglob("*"))
        if path.is_file()
    }
    host_tag = "rpi5-final-rc-v6"
    host_sha = "d4f26e32e6601419a0768403b8a0728035ff1e67"
    releases = {}
    controls = {}
    for item in selected.values():
        releases[item.release_tag] = releases.get(item.release_tag, 0) + 1
        controls[item.control_generation] = controls.get(item.control_generation, 0) + 1
    return ExpandedN5Selection(
        layout=current,
        batch_id="expanded-n5-fixture",
        manifest_sha256="a" * 64,
        composite_sha256="b" * 64,
        selected=selected,
        aliases={},
        prerequisites=(
            {
                "result_key": "e-host-thermal-storage/eight-phase-load-ladder/run-01",
                "source_leaf": host.relative_to(current.volume).as_posix(),
                "source_tag": host_tag,
                "source_git_sha": host_sha,
                "control_generation": "B00-prerequisite",
            },
        ),
        unselected_attempts=(),
        releases=dict(sorted(releases.items())),
        controls=dict(sorted(controls.items())),
        _manifest=manifest,
    )


def test_builds_and_renders_all_completed_families_without_mutating_sources(
    tmp_path: Path,
) -> None:
    selection = analysis_selection_fixture(tmp_path)
    before = {
        root: {
            path.relative_to(root).as_posix(): hashlib.sha256(path.read_bytes()).hexdigest()
            for path in sorted(root.rglob("*"))
            if path.is_file()
        }
        for root in (selection.layout.raw, selection.layout.manifests)
    }

    datasets, supporting = build_expanded_n5_datasets(selection)
    assert set(datasets) == set(REQUIRED_FAMILIES)
    capacity = datasets["capacity-knee"]
    assert capacity.loc[
        (capacity.system == "mqtt-loopback") & (capacity.offered_rate_msg_s == 8_000),
        "classification",
    ].eq("bad").all()
    assert capacity.loc[
        capacity.system.isin(("native", "wafer"))
        & (capacity.offered_rate_msg_s >= 8_000),
        "classification",
    ].eq("support-confounded").all()
    assert capacity.loc[
        (capacity.system == "mqtt-loopback") & (capacity.offered_rate_msg_s < 8_000),
        "classification",
    ].eq("good").all()
    swap = datasets["swap-10ms-timeline"]
    burst = swap[swap.source_result_key.str.startswith("e-swap-4/")]
    assert not burst["drain_right_censored"].any()
    observations = {
        family: f"Observed completed N=5 values for {family}."
        for family in REQUIRED_FAMILIES
    }
    manifest = completed_visual_manifest(datasets, observations)
    validate_expanded_n5_datasets(datasets, selection, manifest)
    derived, reports = render_expanded_n5_results(
        datasets,
        selection,
        observations,
        analyzer_git_sha="e" * 40,
        analyzer_tag="rpi5-final-rc-v15",
        supporting_tables=supporting,
    )

    assert all(
        before[root]
        == {
            path.relative_to(root).as_posix(): hashlib.sha256(path.read_bytes()).hexdigest()
            for path in sorted(root.rglob("*"))
            if path.is_file()
        }
        for root in before
    )
    for extension in ("csv", "svg", "png", "pdf", "html"):
        assert len(list(derived.glob(f"*.{extension}"))) >= 12
    report = (reports / "index.html").read_text()
    assert "DIAGNOSTIC N=5 REVIEW — NOT THESIS EVIDENCE" in report
    assert "E-Perf-5" in report and "PENDING" in report
    family_text = "\n".join(
        (derived / f"{family}.{format_name}").read_text(errors="ignore")
        for family in REQUIRED_FAMILIES
        for format_name in ("csv", "html")
    )
    assert "PRE-RESULTS" not in family_text.upper()
    assert "PENDING" not in family_text.upper()
    validate_enhanced_visual_artifacts(derived, reports, manifest)
    artifact_manifest = json.loads((derived / "artifact-manifest.json").read_text())
    observations_artifact = derived / artifact_manifest["observations"]
    assert observations_artifact.is_file()
    assert hashlib.sha256(observations_artifact.read_bytes()).hexdigest() == (
        artifact_manifest["observations_sha256"]
    )
    assert artifact_manifest["analyzer_tag"] == "rpi5-final-rc-v15"
    assert artifact_manifest["source_composite_sha256"] == selection.composite_sha256
    assert {item["id"] for item in artifact_manifest["supporting_artifacts"]} == {
        "ekuiper-compatible-pairs",
        "ekuiper-release-sensitivity",
    }

    second = analysis_selection_fixture(tmp_path / "second")
    second_datasets, second_supporting = build_expanded_n5_datasets(second)
    second_derived, second_reports = render_expanded_n5_results(
        second_datasets,
        second,
        observations,
        analyzer_git_sha="e" * 40,
        analyzer_tag="rpi5-final-rc-v15",
        supporting_tables=second_supporting,
    )
    for first_root, second_root in ((derived, second_derived), (reports, second_reports)):
        first_hashes = {
            path.relative_to(first_root).as_posix(): hashlib.sha256(path.read_bytes()).hexdigest()
            for path in sorted(first_root.rglob("*"))
            if path.is_file()
        }
        second_hashes = {
            path.relative_to(second_root).as_posix(): hashlib.sha256(path.read_bytes()).hexdigest()
            for path in sorted(second_root.rglob("*"))
            if path.is_file()
        }
        assert first_hashes == second_hashes


def test_completed_datasets_reject_row_provenance_drift(tmp_path: Path) -> None:
    selection = analysis_selection_fixture(tmp_path)
    datasets, _ = build_expanded_n5_datasets(selection)
    observations = {
        family: f"Observed completed N=5 values for {family}."
        for family in REQUIRED_FAMILIES
    }
    manifest = completed_visual_manifest(datasets, observations)

    changed = {name: frame.copy() for name, frame in datasets.items()}
    changed["swap-10ms-timeline"].loc[
        changed["swap-10ms-timeline"].source_result_key.str.startswith("e-swap-4/"),
        "drain_right_censored",
    ] = True
    with pytest.raises(ValueError, match="right-censored drain"):
        validate_expanded_n5_datasets(changed, selection, manifest)

    for field, value, message in (
        ("source_result_key", "unknown/run-01", "not selected by composite"),
        ("source_tag", "rpi5-final-rc-v15", "lineage differs"),
        ("source_git_sha", "f" * 40, "lineage differs"),
        ("source_dirty", True, "dirty source evidence"),
    ):
        changed = {name: frame.copy() for name, frame in datasets.items()}
        changed["payload-knee"].loc[0, field] = value
        with pytest.raises(ValueError, match=message):
            validate_expanded_n5_datasets(changed, selection, manifest)


def test_release_confounded_pairs_are_excluded_and_stratified() -> None:
    rows = pd.DataFrame(
        [
            {"rate_msg_s": 1000, "run_index": 1, "arm": "profiled", "value": 12.0, "source_tag": "rpi5-final-rc-v12"},
            {"rate_msg_s": 1000, "run_index": 1, "arm": "control", "value": 10.0, "source_tag": "rpi5-final-rc-v12"},
            {"rate_msg_s": 1000, "run_index": 2, "arm": "profiled", "value": 14.0, "source_tag": "rpi5-final-rc-v13"},
            {"rate_msg_s": 1000, "run_index": 2, "arm": "control", "value": 11.0, "source_tag": "rpi5-final-rc-v12"},
        ]
    )

    arms, paired, sensitivity = classify_release_pairs(
        rows,
        pair_columns=("rate_msg_s", "run_index"),
        arm_column="arm",
        value_column="value",
        arms=("profiled", "control"),
    )

    assert arms.loc[arms.run_index == 2, "pair_status"].eq("release-confounded").all()
    assert paired[["rate_msg_s", "run_index", "difference"]].to_dict("records") == [
        {"rate_msg_s": 1000, "run_index": 1, "difference": 2.0}
    ]
    assert set(sensitivity["source_tag"]) == {"rpi5-final-rc-v12", "rpi5-final-rc-v13"}
