"""Composite-authorized access to the mixed-lineage expanded N=5 evidence."""

from __future__ import annotations

import argparse
import copy
import hashlib
import json
import re
import sys
from collections import Counter
from dataclasses import dataclass
from pathlib import Path, PurePosixPath
from typing import Any

import pandas as pd

from .enhanced_visuals import (
    REQUIRED_FAMILIES,
    load_enhanced_visual_manifest,
    render_completed_n5_visual_suite,
    validate_completed_visual_manifest,
    validate_enhanced_visual_datasets,
)
from .results_layout import ResultsLayout

ALLOWED_EVIDENCE_RELEASES = {
    "rpi5-final-rc-v10": "c121b49e0a5dfaee7822269fb838c415f8d29ba2",
    "rpi5-final-rc-v11": "b79b6858f3b40416b2f92c27d38213261f5a9a4c",
    "rpi5-final-rc-v12": "7074b33a71da8646e520b3a85fa77e9b83b8453e",
    "rpi5-final-rc-v13": "55b1a4e942c192908981a8aa970c180829fd6360",
}
ALLOWED_CONTROL_GENERATIONS = {
    "legacy-v10",
    "continuation-01",
    "continuation-03",
    "continuation-05",
    "continuation-08",
    "continuation-09",
    "continuation-10",
}
PRODUCTION_COUNTS = {
    "schedule_records": 661,
    "selected_raw_leaves": 623,
    "aliases": 37,
    "qualified_prerequisites": 1,
    "unselected_attempts": 4,
    "unselected_failed_attempts": 4,
}


def _sha256(path: Path) -> str:
    digest = hashlib.sha256()
    with path.open("rb") as stream:
        for chunk in iter(lambda: stream.read(1024 * 1024), b""):
            digest.update(chunk)
    return digest.hexdigest()


def _object(path: Path, label: str) -> dict[str, Any]:
    try:
        value = json.loads(path.read_text(encoding="utf-8"))
    except (OSError, json.JSONDecodeError) as error:
        raise ValueError(f"cannot read {label}: {error}") from error
    if not isinstance(value, dict):
        raise ValueError(f"{label} must be a JSON object")
    return value


def _relative_file(layout: ResultsLayout, value: str, root: Path, label: str) -> Path:
    relative = PurePosixPath(value)
    if relative.is_absolute() or not relative.parts or ".." in relative.parts:
        raise ValueError(f"{label} has an unsafe path: {value}")
    path = layout.volume.joinpath(*relative.parts)
    resolved = path.resolve(strict=False)
    root_resolved = root.resolve()
    if root_resolved not in (resolved, *resolved.parents):
        raise ValueError(f"{label} is outside {root.name}: {value}")
    if not path.is_file() or path.is_symlink():
        raise ValueError(f"{label} is missing or linked: {value}")
    return path


def _manifest_entries(path: Path) -> dict[str, str]:
    entries: dict[str, str] = {}
    for number, line in enumerate(path.read_text(encoding="utf-8").splitlines(), 1):
        digest, separator, relative = line.partition("  ")
        candidate = PurePosixPath(relative)
        if (
            not separator
            or len(digest) != 64
            or any(character not in "0123456789abcdef" for character in digest)
            or candidate.is_absolute()
            or not candidate.parts
            or candidate.parts[0] != "raw"
            or ".." in candidate.parts
            or relative in entries
        ):
            raise ValueError(f"raw manifest line {number} is invalid")
        entries[relative] = digest
    if not entries:
        raise ValueError("raw manifest is empty")
    return entries


def _verify_raw_manifest(layout: ResultsLayout, entries: dict[str, str]) -> None:
    observed = {
        path.relative_to(layout.volume).as_posix()
        for path in layout.raw.rglob("*")
        if path.is_file()
    }
    if observed != set(entries):
        raise ValueError("raw tree path set differs from source manifest")
    for relative, digest in sorted(entries.items()):
        path = _relative_file(layout, relative, layout.raw, "raw manifest entry")
        if path.stat().st_nlink > 1 or _sha256(path) != digest:
            raise ValueError(f"raw manifest entry differs: {relative}")


@dataclass(frozen=True)
class SelectedEvidence:
    result_key: str
    path: str
    release_tag: str
    wafer_git_sha: str
    control_generation: str
    control_batch: str
    metadata_sha256: str
    status_sha256: str


@dataclass(frozen=True)
class ExpandedN5Selection:
    layout: ResultsLayout
    batch_id: str
    manifest_sha256: str
    composite_sha256: str
    selected: dict[str, SelectedEvidence]
    aliases: dict[str, dict[str, Any]]
    prerequisites: tuple[dict[str, Any], ...]
    unselected_attempts: tuple[dict[str, Any], ...]
    releases: dict[str, int]
    controls: dict[str, int]

    def leaf(self, result_key: str) -> Path:
        try:
            selected = self.selected[result_key]
        except KeyError as error:
            raise ValueError(f"result key is not selected by composite: {result_key}") from error
        path = self.layout.resolve_raw_relative(selected.path)
        if not path.is_dir() or path.is_symlink():
            raise ValueError(f"selected leaf is missing or linked: {result_key}")
        return path

    def artifact(self, result_key: str, name: str) -> Path:
        relative_name = PurePosixPath(name)
        if relative_name.is_absolute() or ".." in relative_name.parts:
            raise ValueError(f"artifact name is unsafe: {name}")
        path = self.leaf(result_key).joinpath(*relative_name.parts)
        relative = self.layout.relative(path)
        digest = self._manifest.get(relative)
        if digest is None:
            raise ValueError(f"artifact is not selected by raw manifest: {relative}")
        if not path.is_file() or path.is_symlink() or _sha256(path) != digest:
            raise ValueError(f"selected artifact digest differs: {relative}")
        return path

    _manifest: dict[str, str]


def load_expanded_n5_selection(
    layout: ResultsLayout,
    composite_path: Path,
    source_seal_path: Path,
    manifest_path: Path,
    handoff_path: Path,
    *,
    expected_counts: dict[str, int] | None = None,
) -> ExpandedN5Selection:
    if not layout.explicit:
        raise ValueError("expanded N=5 analysis requires an explicit results volume")
    layout.validate()
    composite_path = _relative_file(layout, layout.relative(composite_path), layout.manifests, "composite index")
    source_seal_path = _relative_file(layout, layout.relative(source_seal_path), layout.manifests, "source seal")
    manifest_path = _relative_file(layout, layout.relative(manifest_path), layout.manifests, "raw manifest")
    handoff_path = _relative_file(layout, layout.relative(handoff_path), layout.manifests, "handoff receipt")
    composite = _object(composite_path, "composite index")
    source_seal = _object(source_seal_path, "source seal")
    manifest = _manifest_entries(manifest_path)
    _verify_raw_manifest(layout, manifest)
    manifest_sha256 = _sha256(manifest_path)
    composite_sha256 = _sha256(composite_path)
    handoff = _object(handoff_path, "handoff receipt")
    expected = expected_counts or PRODUCTION_COUNTS
    if (
        composite.get("schema_version") != 1
        or composite.get("classification") != "diagnostic-expanded-n5-composite"
        or composite.get("campaign_started") is not False
        or composite.get("thesis_evidence") is not False
        or composite.get("n30_admitted") is not False
        or composite.get("selection_rule")
        != "exact terminal reconciliation selection; no attempt pooling"
        or composite.get("counts") != expected
        or composite.get("raw_manifest_sha256") != manifest_sha256
        or source_seal.get("state") != "source-host-sealed"
        or source_seal.get("batch_id") != composite.get("batch_id")
        or source_seal.get("manifest_sha256") != manifest_sha256
        or source_seal.get("composite_sha256") != composite_sha256
        or handoff.get("state") != "handoff-verified"
        or handoff.get("host") != "macos"
        or handoff.get("batch_id") != composite.get("batch_id")
        or handoff.get("manifest_sha256") != manifest_sha256
        or handoff.get("source_seal_sha256") != _sha256(source_seal_path)
        or handoff.get("composite_sha256") != composite_sha256
    ):
        raise ValueError("expanded N=5 source seal or composite identity is invalid")
    selected_values = composite.get("selected")
    aliases = composite.get("aliases")
    prerequisites = composite.get("qualified_prerequisites")
    unselected = composite.get("unselected_attempts")
    if not isinstance(selected_values, dict) or len(selected_values) != expected["selected_raw_leaves"]:
        raise ValueError("composite selected population differs")
    if not isinstance(aliases, dict) or len(aliases) != expected["aliases"]:
        raise ValueError("composite alias population differs")
    if not isinstance(prerequisites, list) or len(prerequisites) != expected["qualified_prerequisites"]:
        raise ValueError("composite prerequisite population differs")
    if not isinstance(unselected, list) or len(unselected) != expected["unselected_attempts"]:
        raise ValueError("composite unselected population differs")
    selected: dict[str, SelectedEvidence] = {}
    releases: Counter[str] = Counter()
    controls: Counter[str] = Counter()
    selected_paths: set[str] = set()
    for result_key, item in selected_values.items():
        if not isinstance(item, dict):
            raise ValueError(f"selected record is invalid: {result_key}")
        path_value = str(item.get("path", ""))
        if path_value in selected_paths:
            raise ValueError("composite pools one raw leaf across selected records")
        selected_paths.add(path_value)
        metadata_path = _relative_file(layout, f"{path_value}/metadata.json", layout.raw, "selected metadata")
        status_path = _relative_file(layout, f"{path_value}/canonical-status.json", layout.raw, "selected status")
        if manifest.get(layout.relative(metadata_path)) != item.get("metadata_sha256") or _sha256(metadata_path) != item.get("metadata_sha256"):
            raise ValueError(f"selected metadata digest differs: {result_key}")
        if manifest.get(layout.relative(status_path)) != item.get("status_sha256") or _sha256(status_path) != item.get("status_sha256"):
            raise ValueError(f"selected status digest differs: {result_key}")
        metadata = _object(metadata_path, "selected metadata")
        status = _object(status_path, "selected status")
        rehearsal = metadata.get("expanded_n5_rehearsal")
        if not isinstance(rehearsal, dict):
            raise ValueError(f"selected metadata lacks expanded N=5 lineage: {result_key}")
        tag = str(item.get("release_tag", ""))
        sha = str(item.get("wafer_git_sha", ""))
        control = str(item.get("control_generation", ""))
        if (
            ALLOWED_EVIDENCE_RELEASES.get(tag) != sha
            or tag in {
                "rpi5-final-rc-v14",
                "rpi5-final-rc-v15",
                "rpi5-final-rc-v16",
                "rpi5-final-rc-v17",
                "rpi5-final-rc-v18",
            }
            or control not in ALLOWED_CONTROL_GENERATIONS
            or metadata.get("git_dirty") is not False
            or metadata.get("git_sha") != sha
            or metadata.get("git_tags") != [tag]
            or rehearsal.get("batch_id") != composite["batch_id"]
            or rehearsal.get("release_tag") != tag
            or rehearsal.get("wafer_git_sha") != sha
            or rehearsal.get("control_generation", "legacy-v10") != control
            or rehearsal.get("pool_with_final_campaign", False) is not False
            or status.get("status") != "passed"
        ):
            raise ValueError(f"selected evidence lineage differs: {result_key}")
        selected[result_key] = SelectedEvidence(
            result_key=result_key,
            path=path_value,
            release_tag=tag,
            wafer_git_sha=sha,
            control_generation=control,
            control_batch=str(item.get("control_batch", "")),
            metadata_sha256=str(item.get("metadata_sha256", "")),
            status_sha256=str(item.get("status_sha256", "")),
        )
        releases[tag] += 1
        controls[control] += 1
    if dict(sorted(releases.items())) != composite.get("release_composition"):
        raise ValueError("composite release composition differs")
    if dict(sorted(controls.items())) != composite.get("control_composition"):
        raise ValueError("composite control composition differs")
    for result_key, item in aliases.items():
        if not isinstance(item, dict):
            raise ValueError(f"composite alias is invalid: {result_key}")
        receipt = _relative_file(
            layout, str(item.get("path", "")), layout.manifests, "alias receipt"
        )
        source = selected.get(str(item.get("source_result_key", "")))
        if (
            _sha256(receipt) != item.get("receipt_sha256")
            or source is None
            or source.path != item.get("source_leaf")
            or source.status_sha256 != item.get("source_status_sha256")
        ):
            raise ValueError(f"composite alias binding differs: {result_key}")
    for item in prerequisites:
        if not isinstance(item, dict) or item.get("result_key") != (
            "e-host-thermal-storage/eight-phase-load-ladder/run-01"
        ):
            raise ValueError("composite prerequisite is invalid")
        source_leaf = str(item.get("source_leaf", ""))
        for name, digest in item.get("source_artifacts", {}).items():
            artifact = _relative_file(
                layout, f"{source_leaf}/{name}", layout.raw, "prerequisite artifact"
            )
            if manifest.get(layout.relative(artifact)) != digest or _sha256(artifact) != digest:
                raise ValueError(f"prerequisite artifact digest differs: {name}")
    unselected_paths = [str(item.get("path", "")) for item in unselected]
    if len(unselected_paths) != len(set(unselected_paths)) or selected_paths & set(unselected_paths):
        raise ValueError("composite selected and unselected paths are not disjoint")
    for item in unselected:
        status = _relative_file(
            layout,
            f"{item.get('path', '')}/canonical-status.json",
            layout.raw,
            "unselected status",
        )
        if item.get("status") not in {"failed", "interrupted"} or _sha256(status) != item.get(
            "status_sha256"
        ):
            raise ValueError("composite unselected attempt differs")
    return ExpandedN5Selection(
        layout=layout,
        batch_id=str(composite["batch_id"]),
        manifest_sha256=manifest_sha256,
        composite_sha256=composite_sha256,
        selected=selected,
        aliases=dict(aliases),
        prerequisites=tuple(prerequisites),
        unselected_attempts=tuple(unselected),
        releases=dict(sorted(releases.items())),
        controls=dict(sorted(controls.items())),
        _manifest=manifest,
    )


def _selected_parts(result_key: str) -> tuple[str, str, int]:
    experiment, remainder = result_key.split("/", 1)
    condition, run = remainder.rsplit("/", 1)
    match = re.fullmatch(r"run-(\d+)", run)
    if match is None:
        raise ValueError(f"selected result key has invalid run identity: {result_key}")
    return experiment, condition, int(match.group(1))


def _value(value: Any, field: str) -> float | None:
    if isinstance(value, dict):
        if value.get("status") != "available":
            return None
        value = value.get("value")
    if not isinstance(value, (int, float)):
        raise ValueError(f"metric field is invalid: {field}")
    return float(value)


def _artifact_json(selection: ExpandedN5Selection, key: str, name: str) -> tuple[dict[str, Any], Path]:
    path = selection.artifact(key, name)
    return _object(path, f"{key} {name}"), path


def _row(
    selection: ExpandedN5Selection,
    result_key: str,
    artifact: Path,
    *,
    family: dict[str, Any],
    sample_id: str,
    metric: str,
    value: float,
    unit: str,
    x_value: float,
    **extra: Any,
) -> dict[str, Any]:
    selected = selection.selected[result_key]
    experiment, condition, run_index = _selected_parts(result_key)
    relative = selection.layout.relative(artifact)
    return {
        "experiment": experiment,
        "condition": condition,
        "run_index": run_index,
        "sample_id": sample_id,
        "metric": metric,
        "value": value,
        "unit": unit,
        "x_value": x_value,
        "source_git_sha": selected.wafer_git_sha,
        "source_tag": selected.release_tag,
        "source_dirty": False,
        "batch_id": selection.batch_id,
        "evidence_class": family["evidence_class"],
        "thesis_evidence": False,
        "n30_admitted": False,
        "sample_unit": family["sample_unit"],
        "source_result_key": result_key,
        "control_generation": selected.control_generation,
        "source_relative_path": relative,
        "source_sha256": selection._manifest[relative],
        **extra,
    }


def _records(selection: ExpandedN5Selection, experiment: str) -> list[tuple[str, SelectedEvidence]]:
    records = [
        (key, selected)
        for key, selected in selection.selected.items()
        if key.startswith(f"{experiment}/")
    ]
    return sorted(records)


def build_expanded_n5_datasets(
    selection: ExpandedN5Selection,
) -> tuple[dict[str, pd.DataFrame], dict[str, pd.DataFrame]]:
    definitions = {
        item["id"]: item for item in load_enhanced_visual_manifest()["families"]
    }
    rows: dict[str, list[dict[str, Any]]] = {family: [] for family in REQUIRED_FAMILIES}

    for key, _ in _records(selection, "e-perf-1"):
        interval, interval_path = _artifact_json(selection, key, "interval-metrics.json")
        for index, item in enumerate(interval.get("rows", [])):
            common = {
                "interval_index": index,
                "interval_start_ns": item.get("interval_start_ns"),
                "interval_end_ns": item.get("interval_end_ns"),
                "expected_interval_rows": interval.get("row_count"),
            }
            p99 = _value(item.get("latency_p99_ns"), "latency_p99_ns")
            if p99 is not None:
                rows["interval-latency-throughput"].append(
                    _row(
                        selection,
                        key,
                        interval_path,
                        family=definitions["interval-latency-throughput"],
                        sample_id=f"interval-{index:03d}-p99",
                        metric="latency-p99",
                        value=p99,
                        unit="nanoseconds",
                        x_value=float(index),
                        **common,
                    )
                )
            rows["interval-latency-throughput"].append(
                _row(
                    selection,
                    key,
                    interval_path,
                    family=definitions["interval-latency-throughput"],
                    sample_id=f"interval-{index:03d}-throughput",
                    metric="throughput",
                    value=float(item["throughput_messages_per_second"]),
                    unit="messages/second",
                    x_value=float(index),
                    **common,
                )
            )
            power = _value(
                item.get("pmic_internal_rail_proxy_watts"),
                "pmic_internal_rail_proxy_watts",
            )
            if power is not None and power > 0:
                rows["pmic-proxy-efficiency"].append(
                    _row(
                        selection,
                        key,
                        interval_path,
                        family=definitions["pmic-proxy-efficiency"],
                        sample_id=f"interval-{index:03d}-efficiency",
                        metric="delivered-throughput-per-pmic-proxy-watt",
                        value=float(item["throughput_messages_per_second"]) / power,
                        unit="messages/second per PMIC internal-rail proxy watt",
                        x_value=float(index),
                        **common,
                    )
                )

    capacity_runs: list[tuple[str, dict[str, Any], Path]] = []
    for key, _ in _records(selection, "e-perf-capacity-knee"):
        value, path = _artifact_json(selection, key, "capacity-run.json")
        capacity_runs.append((key, value, path))
    grouped: dict[tuple[str, int], list[dict[str, Any]]] = {}
    for _, value, _ in capacity_runs:
        grouped.setdefault((str(value["system"]), int(value["rate_msg_s"])), []).append(value)
    mqtt_bad = sorted(
        rate
        for (system, rate), values in grouped.items()
        if system == "mqtt-loopback"
        and (
            sum(int(value["messages"]["total_undelivered"]) for value in values)
            / sum(int(value["messages"]["intended"]) for value in values)
            > 0.01
            or sum(float(value["rates_msg_s"]["achieved_ratio"]) for value in values)
            / len(values)
            < 0.99
            or sum(int(value["messages"]["duplicates"]) for value in values)
            != 0
        )
    )
    support_bad = mqtt_bad[0] if mqtt_bad else None
    classifications: dict[tuple[str, int], str] = {}
    for identity, values in grouped.items():
        system, rate = identity
        intended = sum(int(value["messages"]["intended"]) for value in values)
        undelivered = sum(int(value["messages"]["total_undelivered"]) for value in values)
        pooled_loss = undelivered / intended if intended else 1.0
        mean_ratio = sum(float(value["rates_msg_s"]["achieved_ratio"]) for value in values) / len(values)
        duplicates = sum(int(value["messages"]["duplicates"]) for value in values)
        classification = (
            "good" if pooled_loss <= 0.01 and mean_ratio >= 0.99 and duplicates == 0 else "bad"
        )
        if system != "mqtt-loopback" and support_bad is not None and rate >= support_bad:
            classification = "support-confounded"
        classifications[identity] = classification
    for key, value, path in capacity_runs:
        system = str(value["system"])
        rate = int(value["rate_msg_s"])
        classification = classifications[(system, rate)]
        support_confounded = classification == "support-confounded"
        common = {
            "system": system,
            "offered_rate_msg_s": rate,
            "classification": classification,
            "support_confounded": support_confounded,
            "support_bad_from_rate_msg_s": support_bad,
        }
        messages = value["messages"]
        run_good = (
            int(messages["total_undelivered"]) / int(messages["intended"]) <= 0.01
            and float(value["rates_msg_s"]["achieved_ratio"]) >= 0.99
            and int(messages["duplicates"]) == 0
        )
        rows["capacity-knee"].extend(
            [
                _row(
                    selection,
                    key,
                    path,
                    family=definitions["capacity-knee"],
                    sample_id=f"run-{value['run_index']:02d}-achieved",
                    metric="achieved-rate",
                    value=float(value["rates_msg_s"]["achieved"]),
                    unit="messages/second",
                    x_value=float(rate),
                    **common,
                ),
                _row(
                    selection,
                    key,
                    path,
                    family=definitions["capacity-knee"],
                    sample_id=f"run-{value['run_index']:02d}-loss",
                    metric="loss-fraction",
                    value=float(messages["total_undelivered"]) / int(messages["intended"]),
                    unit="fraction",
                    x_value=float(rate),
                    **common,
                ),
            ]
        )
        rows["delivery-probability"].append(
            _row(
                selection,
                key,
                path,
                family=definitions["delivery-probability"],
                sample_id=f"run-{value['run_index']:02d}",
                metric="delivery-good",
                value=float(run_good),
                unit="proportion",
                x_value=float(rate),
                **common,
            )
        )

    for key, _ in _records(selection, "e-perf-payload-refinement"):
        payload, payload_path = _artifact_json(selection, key, "payload-manifest.json")
        latency, _ = _artifact_json(selection, key, "percentiles.json")
        for percentile in ("p50", "p95", "p99"):
            rows["payload-knee"].append(
                _row(
                    selection,
                    key,
                    payload_path,
                    family=definitions["payload-knee"],
                    sample_id=percentile,
                    metric=f"latency-{percentile}",
                    value=float(latency[f"{percentile}_ns"]),
                    unit="nanoseconds",
                    x_value=float(payload["payload_bytes"]),
                    payload_sha256=payload["payload_sha256"],
                )
            )

    for key, _ in _records(selection, "e-perf-depth-extension"):
        topology, topology_path = _artifact_json(selection, key, "topology-manifest.json")
        latency, _ = _artifact_json(selection, key, "percentiles.json")
        memory_path = selection.artifact(key, "memory.csv")
        memory = pd.read_csv(memory_path)
        rows["extended-depth-latency-rss"].extend(
            [
                _row(
                    selection,
                    key,
                    topology_path,
                    family=definitions["extended-depth-latency-rss"],
                    sample_id="latency-p95",
                    metric="latency-p95",
                    value=float(latency["p95_ns"]),
                    unit="nanoseconds",
                    x_value=float(topology["depth"]),
                ),
                _row(
                    selection,
                    key,
                    memory_path,
                    family=definitions["extended-depth-latency-rss"],
                    sample_id="peak-rss",
                    metric="peak-rss",
                    value=float(memory["rss_bytes"].max()),
                    unit="bytes",
                    x_value=float(topology["depth"]),
                ),
            ]
        )

    for experiment in ("e-swap-3", "e-swap-4"):
        for key, _ in _records(selection, experiment):
            timeline, path = _artifact_json(selection, key, "throughput-buckets-10ms.json")
            drain_censored = False
            if experiment == "e-swap-4":
                burst, _ = _artifact_json(selection, key, "burst-timeline.json")
                drain_censored = bool(burst["drain_right_censored"])
            for index, bucket in enumerate(timeline["buckets"]):
                rows["swap-10ms-timeline"].append(
                    _row(
                        selection,
                        key,
                        path,
                        family=definitions["swap-10ms-timeline"],
                        sample_id=f"bucket-{index:03d}",
                        metric="throughput",
                        value=float(bucket["rate_msg_s"]),
                        unit="messages/second",
                        x_value=float(bucket["start_offset_ns"]) / 1_000_000,
                        bucket_index=index,
                        bucket_start_ns=int(bucket["start_offset_ns"]),
                        bucket_end_ns=int(bucket["end_offset_ns"]),
                        drain_right_censored=drain_censored,
                    )
                )

    for experiment, family_id, artifact_name, metric_name in (
        ("e-swap-independent-sessions", "swap-hierarchy", "hotswap-analysis.json", "sink-gap"),
        ("e-swap-rollback-sessions", "rollback-hierarchy", "rollback.json", "rollback-duration"),
    ):
        value_field = "sink_observed_output_gap_ns" if family_id == "swap-hierarchy" else "rollback_ns"
        for key, _ in _records(selection, experiment):
            summary, path = _artifact_json(selection, key, artifact_name)
            classes = Counter(str(event["event_class"]) for event in summary["events"])
            for event in summary["events"]:
                rows[family_id].append(
                    _row(
                        selection,
                        key,
                        path,
                        family=definitions[family_id],
                        sample_id=f"event-{int(event['event_index']):02d}",
                        metric=metric_name,
                        value=float(event[value_field]),
                        unit="nanoseconds",
                        x_value=float(event["event_index"]),
                        event_class=event["event_class"],
                        event_count=classes[event["event_class"]],
                    )
                )

    prerequisite = selection.prerequisites[0]
    prerequisite_key = str(prerequisite["result_key"])
    prerequisite_root = selection.layout.volume / str(prerequisite["source_leaf"])
    host_tag = str(prerequisite["source_tag"])
    host_sha = str(prerequisite["source_git_sha"])
    for family_id, artifact_name in (
        ("thermal-load-ladder", "host-telemetry.csv"),
        ("usb-io-integrity", "usb-integrity.json"),
    ):
        artifact = prerequisite_root / artifact_name
        relative = selection.layout.relative(artifact)
        if selection._manifest.get(relative) != _sha256(artifact):
            raise ValueError(f"prerequisite artifact digest differs: {artifact_name}")
        if family_id == "thermal-load-ladder":
            telemetry = pd.read_csv(artifact)
            for index, item in telemetry.iterrows():
                for metric, field, unit in (
                    ("temperature", "temperature_millicelsius", "degrees Celsius"),
                    ("cpu-frequency", "cpu_frequency_hz", "hertz"),
                    ("memory-pressure", "memory_psi_some_avg10", "pressure ratio"),
                ):
                    rows[family_id].append(
                        {
                            "experiment": "e-host-thermal-storage",
                            "condition": str(item["phase"]),
                            "run_index": 1,
                            "sample_id": f"sample-{index:04d}-{metric}",
                            "metric": metric,
                            "value": float(item[field]) / (1000 if metric == "temperature" else 1),
                            "unit": unit,
                            "x_value": float(item["phase_elapsed_seconds"]),
                            "source_git_sha": host_sha,
                            "source_tag": host_tag,
                            "source_dirty": False,
                            "batch_id": selection.batch_id,
                            "evidence_class": definitions[family_id]["evidence_class"],
                            "thesis_evidence": False,
                            "n30_admitted": False,
                            "sample_unit": definitions[family_id]["sample_unit"],
                            "source_result_key": prerequisite_key,
                            "control_generation": "B00-prerequisite",
                            "source_relative_path": relative,
                            "source_sha256": selection._manifest[relative],
                        }
                    )
        else:
            integrity = _object(artifact, "USB integrity")
            for index, item in enumerate(integrity["checks"]):
                throughput = (
                    float(item["bytes"]) / float(item["elapsed_seconds"])
                    if float(item["elapsed_seconds"]) > 0
                    else 0.0
                )
                rows[family_id].append(
                    {
                        "experiment": "e-host-thermal-storage",
                        "condition": str(item["phase"]),
                        "run_index": 1,
                        "sample_id": f"phase-{index:02d}",
                        "metric": "verified-throughput",
                        "value": throughput,
                        "unit": "bytes/second",
                        "x_value": float(index),
                        "source_git_sha": host_sha,
                        "source_tag": host_tag,
                        "source_dirty": False,
                        "batch_id": selection.batch_id,
                        "evidence_class": definitions[family_id]["evidence_class"],
                        "thesis_evidence": False,
                        "n30_admitted": False,
                        "sample_unit": definitions[family_id]["sample_unit"],
                        "source_result_key": prerequisite_key,
                        "control_generation": "B00-prerequisite",
                        "source_relative_path": relative,
                        "source_sha256": selection._manifest[relative],
                    }
                )

    ekuiper_pairs = []
    for key, _ in _records(selection, "e-compare-ekuiper-profile"):
        summary, path = _artifact_json(selection, key, "ekuiper-runtime-summary.json")
        for metric in ("p95", "p99"):
            ekuiper_pairs.append(
                _row(
                    selection,
                    key,
                    path,
                    family=definitions["ekuiper-tail-association"],
                    sample_id=metric,
                    metric=f"latency-{metric}",
                    value=float(summary["latency_ns"][metric]),
                    unit="nanoseconds",
                    x_value=float(summary["rate_msg_s"]),
                    rate_msg_s=int(summary["rate_msg_s"]),
                    arm=str(summary["profiler_state"]),
                )
            )
    ekuiper = pd.DataFrame(ekuiper_pairs)
    arm_rows, paired, sensitivity = classify_release_pairs(
        ekuiper.loc[ekuiper["metric"] == "latency-p99"],
        pair_columns=("rate_msg_s", "run_index"),
        arm_column="arm",
        value_column="value",
        arms=("profiled", "unprofiled-control"),
    )
    pair_status = arm_rows.set_index(["rate_msg_s", "run_index", "arm"])["pair_status"]
    ekuiper["pair_status"] = [
        pair_status.loc[row.rate_msg_s, row.run_index, row.arm]
        for row in ekuiper.itertuples(index=False)
    ]
    rows["ekuiper-tail-association"] = ekuiper.to_dict("records")

    datasets = {family: pd.DataFrame(values) for family, values in rows.items()}
    return datasets, {
        "ekuiper-compatible-pairs": paired,
        "ekuiper-release-sensitivity": sensitivity,
    }


def completed_visual_manifest(
    datasets: dict[str, pd.DataFrame], observations: dict[str, str]
) -> dict[str, Any]:
    if set(observations) != set(REQUIRED_FAMILIES):
        raise ValueError("completed observations differ from required visual families")
    manifest = copy.deepcopy(load_enhanced_visual_manifest())
    manifest["status"] = "completed"
    manifest["external_gaps"] = [
        {
            "experiment": "E-Perf-5",
            "status": "PENDING",
            "reason": "matched x86 execution has not been completed",
        }
    ]
    for family in manifest["families"]:
        family_id = family["id"]
        observation = observations[family_id].strip()
        if not observation or re.search(r"PENDING|PRE-RESULTS", observation, re.IGNORECASE):
            raise ValueError(f"{family_id}: completed observation is stale or empty")
        family["observed_n5_pattern"] = observation
        frame = datasets[family_id]
        composition = (
            frame[["source_result_key", "source_tag"]]
            .drop_duplicates()["source_tag"]
            .value_counts()
        )
        family["release_composition"] = {
            str(tag): int(count) for tag, count in sorted(composition.items())
        }
    validate_completed_visual_manifest(manifest)
    return manifest


def observations_from_datasets(datasets: dict[str, pd.DataFrame]) -> dict[str, str]:
    observations = {}
    for family_id in REQUIRED_FAMILIES:
        frame = datasets[family_id]
        source_runs = frame[
            ["source_result_key", "source_tag", "source_git_sha"]
        ].drop_duplicates()
        metric_ranges = []
        for (metric, unit), group in frame.groupby(["metric", "unit"], sort=True):
            values = pd.to_numeric(group["value"], errors="raise")
            metric_ranges.append(
                f"{metric} ({unit}) {float(values.min()):.6g} to "
                f"{float(values.max()):.6g}, n={len(values)}"
            )
        observations[family_id] = (
            f"Observed {len(source_runs)} source runs across "
            f"{source_runs['source_tag'].nunique()} evidence release(s): "
            + "; ".join(metric_ranges)
            + "."
        )
    return observations


def validate_expanded_n5_datasets(
    datasets: dict[str, pd.DataFrame],
    selection: ExpandedN5Selection,
    manifest: dict[str, Any],
) -> None:
    validate_completed_visual_manifest(manifest)
    if set(datasets) != set(REQUIRED_FAMILIES):
        raise ValueError("expanded N=5 datasets differ from required visual families")
    structural_manifest = copy.deepcopy(manifest)
    structural_manifest["status"] = "pre-results"
    structural = {
        family_id: frame.assign(
            source_git_sha="a" * 40, source_tag="single-release-structural-check"
        )
        for family_id, frame in datasets.items()
    }
    validate_enhanced_visual_datasets(
        structural,
        expected_source_sha="a" * 40,
        expected_source_tag="single-release-structural-check",
        batch_id=selection.batch_id,
        manifest=structural_manifest,
        layout=selection.layout,
    )
    prerequisite_by_key = {
        str(item.get("result_key")): item for item in selection.prerequisites
    }
    for family_id, frame in datasets.items():
        missing = {"source_result_key", "control_generation"} - set(frame.columns)
        if missing:
            raise ValueError(f"{family_id}: missing composite provenance {sorted(missing)}")
        for row in frame.to_dict("records"):
            result_key = str(row["source_result_key"])
            selected = selection.selected.get(result_key)
            if selected is not None:
                selected_experiment, selected_condition, selected_run = _selected_parts(
                    result_key
                )
                expected_prefix = f"{selected.path}/"
                if (
                    row["experiment"] != selected_experiment
                    or row["condition"] != selected_condition
                    or int(row["run_index"]) != selected_run
                    or row["source_tag"] != selected.release_tag
                    or row["source_git_sha"] != selected.wafer_git_sha
                    or row["control_generation"] != selected.control_generation
                ):
                    raise ValueError(f"{family_id}: row lineage differs from composite")
            else:
                prerequisite = prerequisite_by_key.get(result_key)
                if prerequisite is None:
                    raise ValueError(f"{family_id}: row is not selected by composite")
                expected_prefix = f"{prerequisite['source_leaf']}/"
                if (
                    row["source_tag"] != prerequisite["source_tag"]
                    or row["source_git_sha"] != prerequisite["source_git_sha"]
                    or row["control_generation"] != "B00-prerequisite"
                ):
                    raise ValueError(f"{family_id}: prerequisite lineage differs")
            relative = str(row["source_relative_path"])
            if not relative.startswith(expected_prefix):
                raise ValueError(f"{family_id}: source path is outside selected evidence")
            source = _relative_file(
                selection.layout, relative, selection.layout.raw, "source artifact"
            )
            if (
                selection._manifest.get(relative) != row["source_sha256"]
                or _sha256(source) != row["source_sha256"]
            ):
                raise ValueError(f"{family_id}: source artifact digest differs")
    ekuiper = datasets["ekuiper-tail-association"]
    p99 = ekuiper.loc[ekuiper["metric"] == "latency-p99"]
    annotated, _, _ = classify_release_pairs(
        p99,
        pair_columns=("rate_msg_s", "run_index"),
        arm_column="arm",
        value_column="value",
        arms=("profiled", "unprofiled-control"),
    )
    expected_status = annotated.set_index(["rate_msg_s", "run_index", "arm"])[
        "pair_status"
    ]
    if any(
        row.pair_status != expected_status.loc[row.rate_msg_s, row.run_index, row.arm]
        for row in ekuiper.itertuples(index=False)
    ):
        raise ValueError("ekuiper-tail-association: release-confounded pair status differs")


def render_expanded_n5_results(
    datasets: dict[str, pd.DataFrame],
    selection: ExpandedN5Selection,
    observations: dict[str, str],
    *,
    analyzer_git_sha: str,
    analyzer_tag: str,
    supporting_tables: dict[str, pd.DataFrame] | None = None,
) -> tuple[Path, Path]:
    if re.fullmatch(r"[0-9a-f]{40}", analyzer_git_sha) is None:
        raise ValueError("analyzer Git SHA is invalid")
    if analyzer_tag != "rpi5-final-rc-v18":
        raise ValueError("expanded N=5 analysis requires the signed v18 analyzer")
    manifest = completed_visual_manifest(datasets, observations)
    validate_expanded_n5_datasets(datasets, selection, manifest)
    return render_completed_n5_visual_suite(
        datasets,
        selection.layout,
        batch_id=selection.batch_id,
        manifest=manifest,
        source_manifest_sha256=selection.manifest_sha256,
        source_composite_sha256=selection.composite_sha256,
        analyzer_git_sha=analyzer_git_sha,
        analyzer_tag=analyzer_tag,
        release_composition=selection.releases,
        supporting_tables=supporting_tables,
    )


def classify_release_pairs(
    rows: pd.DataFrame,
    *,
    pair_columns: tuple[str, ...],
    arm_column: str,
    value_column: str,
    arms: tuple[str, str],
) -> tuple[pd.DataFrame, pd.DataFrame, pd.DataFrame]:
    required = {*pair_columns, arm_column, value_column, "source_tag"}
    missing = required - set(rows.columns)
    if missing:
        raise ValueError(f"paired rows lack columns: {sorted(missing)}")
    if set(rows[arm_column]) != set(arms):
        raise ValueError("paired rows have unexpected arms")
    annotated = rows.copy()
    annotated["pair_status"] = ""
    pairs = []
    for identity, group in annotated.groupby(list(pair_columns), sort=True):
        if len(group) != 2 or set(group[arm_column]) != set(arms):
            raise ValueError(f"paired rows are incomplete or duplicated: {identity}")
        compatible = group["source_tag"].nunique() == 1
        annotated.loc[group.index, "pair_status"] = (
            "compatible" if compatible else "release-confounded"
        )
        if compatible:
            by_arm = group.set_index(arm_column)
            values = {column: value for column, value in zip(pair_columns, identity if isinstance(identity, tuple) else (identity,), strict=True)}
            pairs.append(
                {
                    **values,
                    "source_tag": str(group.iloc[0]["source_tag"]),
                    "difference": float(by_arm.loc[arms[0], value_column])
                    - float(by_arm.loc[arms[1], value_column]),
                }
            )
    sensitivity = (
        annotated.groupby(["source_tag", arm_column], sort=True)[value_column]
        .agg(N="count", median="median", minimum="min", maximum="max")
        .reset_index()
    )
    return (
        annotated.reset_index(drop=True),
        pd.DataFrame(pairs, columns=[*pair_columns, "source_tag", "difference"]),
        sensitivity,
    )


def parser() -> argparse.ArgumentParser:
    value = argparse.ArgumentParser(
        description="Render the composite-authorized expanded N=5 diagnostic suite"
    )
    value.add_argument("--results-root", type=Path, required=True)
    value.add_argument("--manifest", type=Path, required=True)
    value.add_argument("--composite", type=Path, required=True)
    value.add_argument("--source-seal", type=Path, required=True)
    value.add_argument("--handoff-receipt", type=Path, required=True)
    value.add_argument("--analyzer-git-sha", required=True)
    value.add_argument("--analyzer-tag", required=True)
    return value


def main() -> int:
    args = parser().parse_args()
    try:
        layout = ResultsLayout.resolve(Path.cwd(), args.results_root)
        selection = load_expanded_n5_selection(
            layout,
            args.composite,
            args.source_seal,
            args.manifest,
            args.handoff_receipt,
        )
        datasets, supporting = build_expanded_n5_datasets(selection)
        observations = observations_from_datasets(datasets)
        derived, reports = render_expanded_n5_results(
            datasets,
            selection,
            observations,
            analyzer_git_sha=args.analyzer_git_sha,
            analyzer_tag=args.analyzer_tag,
            supporting_tables=supporting,
        )
    except (OSError, ValueError, KeyError, TypeError) as error:
        print(f"ERROR: {error}", file=sys.stderr)
        return 1
    print(json.dumps({"derived": str(derived), "reports": str(reports)}, sort_keys=True))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
