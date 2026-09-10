"""Fail-closed rendering for the enhanced N=5 visual suite."""

from __future__ import annotations

import csv
import hashlib
import html
import json
import math
import os
import re
from html.parser import HTMLParser
from pathlib import Path, PurePosixPath
from typing import Any

import matplotlib

matplotlib.use("Agg")
import matplotlib.pyplot as plt
import pandas as pd

from .results_layout import CANONICAL_ALIASES, ResultsLayout

MANIFEST_PATH = Path(__file__).resolve().parents[2] / "enhanced-visual-manifest.json"
REQUIRED_FAMILIES = (
    "interval-latency-throughput",
    "capacity-knee",
    "delivery-probability",
    "payload-knee",
    "extended-depth-latency-rss",
    "swap-10ms-timeline",
    "swap-hierarchy",
    "rollback-hierarchy",
    "thermal-load-ladder",
    "usb-io-integrity",
    "ekuiper-tail-association",
    "pmic-proxy-efficiency",
)
REQUIRED_DEFINITION_FIELDS = {
    "id",
    "title",
    "experiments",
    "evidence_class",
    "source_artifacts",
    "source_fields",
    "units",
    "sample_unit",
    "independent_n",
    "how_to_read",
    "observed_n5_pattern",
    "limits",
}
REQUIRED_DATA_COLUMNS = {
    "experiment",
    "condition",
    "run_index",
    "sample_id",
    "metric",
    "value",
    "unit",
    "x_value",
    "source_git_sha",
    "source_tag",
    "source_dirty",
    "batch_id",
    "evidence_class",
    "thesis_evidence",
    "n30_admitted",
    "sample_unit",
    "source_relative_path",
    "source_sha256",
}
_HEX_SHA = re.compile(r"[0-9a-f]{40}")


class _ReportParser(HTMLParser):
    def __init__(self) -> None:
        super().__init__(convert_charrefs=True)
        self.cards: list[str] = []
        self.links: list[str] = []
        self.headings: list[str] = []
        self._heading: list[str] | None = None

    def handle_starttag(self, tag: str, attrs: list[tuple[str, str | None]]) -> None:
        values = dict(attrs)
        if tag == "article" and values.get("data-visual-family"):
            self.cards.append(str(values["data-visual-family"]))
        if tag == "a" and values.get("href"):
            self.links.append(str(values["href"]))
        if tag in {"h2", "h3"}:
            self._heading = []

    def handle_data(self, data: str) -> None:
        if self._heading is not None:
            self._heading.append(data)

    def handle_endtag(self, tag: str) -> None:
        if tag in {"h2", "h3"} and self._heading is not None:
            self.headings.append("".join(self._heading).strip())
            self._heading = None


def load_enhanced_visual_manifest(path: Path = MANIFEST_PATH) -> dict[str, Any]:
    value = json.loads(path.read_text(encoding="utf-8"))
    validate_enhanced_visual_manifest(value)
    return value


def validate_enhanced_visual_manifest(manifest: dict[str, Any]) -> None:
    if (
        manifest.get("schema_version") != 1
        or manifest.get("status") != "pre-results"
        or manifest.get("classification")
        != "enhanced-n5-candidate-and-diagnostic"
        or manifest.get("thesis_evidence") is not False
        or manifest.get("n30_admitted") is not False
        or manifest.get("campaign_started") is not False
        or manifest.get("artifact_formats") != ["csv", "svg", "png", "pdf", "html"]
    ):
        raise ValueError("enhanced visual manifest identity is invalid")
    if tuple(manifest.get("required_families", ())) != REQUIRED_FAMILIES:
        raise ValueError("enhanced visual manifest family set or order is invalid")
    families = manifest.get("families")
    if not isinstance(families, list) or tuple(
        item.get("id") for item in families if isinstance(item, dict)
    ) != REQUIRED_FAMILIES:
        raise ValueError("enhanced visual definitions differ from required families")
    for item in families:
        missing = REQUIRED_DEFINITION_FIELDS - set(item)
        if missing:
            raise ValueError(f"{item.get('id')}: missing visual fields {sorted(missing)}")
        if any(item[field] in (None, "", []) for field in REQUIRED_DEFINITION_FIELDS):
            raise ValueError(f"{item['id']}: visual fields must not be empty")
        if item["evidence_class"] not in {"candidate-supplementary", "diagnostic"}:
            raise ValueError(f"{item['id']}: invalid evidence class")
        if type(item["independent_n"]) is not int or item["independent_n"] <= 0:
            raise ValueError(f"{item['id']}: invalid independent N")
        if item["id"] == "pmic-proxy-efficiency" and "total input power" not in item["limits"]:
            raise ValueError("PMIC visual must preserve the total-input-power exclusion")
        if item["id"] == "ekuiper-tail-association" and "causality" not in item["limits"]:
            raise ValueError("eKuiper visual must preserve the no-causality boundary")


def validate_completed_visual_manifest(manifest: dict[str, Any]) -> None:
    if (
        manifest.get("schema_version") != 1
        or manifest.get("status") != "completed"
        or manifest.get("classification") != "enhanced-n5-candidate-and-diagnostic"
        or manifest.get("thesis_evidence") is not False
        or manifest.get("n30_admitted") is not False
        or manifest.get("campaign_started") is not False
        or manifest.get("artifact_formats") != ["csv", "svg", "png", "pdf", "html"]
        or tuple(manifest.get("required_families", ())) != REQUIRED_FAMILIES
    ):
        raise ValueError("completed visual manifest identity is invalid")
    families = manifest.get("families")
    if not isinstance(families, list) or tuple(
        item.get("id") for item in families if isinstance(item, dict)
    ) != REQUIRED_FAMILIES:
        raise ValueError("completed visual definitions differ from required families")
    for item in families:
        missing = (REQUIRED_DEFINITION_FIELDS | {"release_composition"}) - set(item)
        if missing:
            raise ValueError(f"{item.get('id')}: missing completed visual fields {sorted(missing)}")
        if any(item[field] in (None, "", []) for field in REQUIRED_DEFINITION_FIELDS):
            raise ValueError(f"{item['id']}: completed visual fields must not be empty")
        text = f"{item['observed_n5_pattern']} {item['limits']}".upper()
        if "PENDING" in text or "PRE-RESULTS" in text:
            raise ValueError(f"{item['id']}: stale pre-results text")


def _definitions(manifest: dict[str, Any]) -> dict[str, dict[str, Any]]:
    return {item["id"]: item for item in manifest["families"]}


def _validate_source_path(value: str, layout: ResultsLayout | None = None) -> Path | None:
    path = PurePosixPath(value)
    if path.is_absolute() or ".." in path.parts or not path.parts:
        raise ValueError(f"invalid source-relative path: {value}")
    if path.parts[0] not in {"raw", "manifests"}:
        raise ValueError(f"source path is outside raw/manifests: {value}")
    if layout is None:
        return None
    resolved = layout.volume.joinpath(*path.parts)
    volume = layout.volume.resolve()
    if volume not in resolved.resolve(strict=False).parents:
        raise ValueError(f"source path escapes results volume: {value}")
    if not resolved.is_file() or resolved.is_symlink():
        raise ValueError(f"source artifact is missing or linked: {value}")
    return resolved


def _validate_interval_rows(family_id: str, frame: pd.DataFrame) -> None:
    if family_id not in {
        "interval-latency-throughput",
        "pmic-proxy-efficiency",
    }:
        return
    required = {
        "interval_index",
        "interval_start_ns",
        "interval_end_ns",
        "expected_interval_rows",
    }
    missing = required - set(frame.columns)
    if missing:
        raise ValueError(f"{family_id}: missing interval fields {sorted(missing)}")
    for identity, group in frame.groupby(
        ["experiment", "condition", "run_index"], sort=False
    ):
        intervals = group[list(required)].drop_duplicates().sort_values(
            "interval_index"
        )
        expected_rows = set(intervals["expected_interval_rows"])
        if len(expected_rows) != 1 or len(intervals) != next(iter(expected_rows)):
            raise ValueError(f"{family_id}: missing or excess intervals for {identity}")
        last_index = len(intervals) - 1
        for expected_index, row in enumerate(intervals.itertuples(index=False)):
            expected_end = (expected_index + 1) * 1_000_000_000
            end = int(row.interval_end_ns)
            if (
                int(row.interval_index) != expected_index
                or int(row.interval_start_ns) != expected_index * 1_000_000_000
                or end <= int(row.interval_start_ns)
                or end > expected_end
                or (expected_index != last_index and end != expected_end)
            ):
                raise ValueError(f"{family_id}: interval grid is not contiguous")


def _validate_fine_swap_rows(family_id: str, frame: pd.DataFrame) -> None:
    if family_id != "swap-10ms-timeline":
        return
    required = {"bucket_index", "bucket_start_ns", "bucket_end_ns"}
    missing = required - set(frame.columns)
    if missing:
        raise ValueError(f"{family_id}: missing fine-window fields {sorted(missing)}")
    for identity, group in frame.groupby(
        ["experiment", "condition", "run_index"], sort=False
    ):
        buckets = group[list(required)].drop_duplicates().sort_values("bucket_index")
        if len(buckets) != 400:
            raise ValueError(f"{family_id}: {identity} requires 400 fine buckets")
        for expected_index, row in enumerate(buckets.itertuples(index=False)):
            start = -2_000_000_000 + expected_index * 10_000_000
            if (
                int(row.bucket_index) != expected_index
                or int(row.bucket_start_ns) != start
                or int(row.bucket_end_ns) != start + 10_000_000
            ):
                raise ValueError(f"{family_id}: fine-window grid is not contiguous")


def _validate_capacity_censoring(family_id: str, frame: pd.DataFrame) -> None:
    if family_id not in {"capacity-knee", "delivery-probability"}:
        return
    required = {
        "system",
        "offered_rate_msg_s",
        "classification",
        "support_confounded",
        "support_bad_from_rate_msg_s",
    }
    missing = required - set(frame.columns)
    if missing:
        raise ValueError(f"{family_id}: missing capacity fields {sorted(missing)}")
    for row in frame.itertuples(index=False):
        support_bad = row.support_bad_from_rate_msg_s
        expected = (
            row.system != "mqtt-loopback"
            and pd.notna(support_bad)
            and float(row.offered_rate_msg_s) >= float(support_bad)
        )
        if bool(row.support_confounded) != expected:
            raise ValueError(f"{family_id}: uncensored capacity row")
        if expected and row.classification != "support-confounded":
            raise ValueError(f"{family_id}: support-confounded row has wrong classification")


def _validate_swap_hierarchy(family_id: str, frame: pd.DataFrame) -> None:
    if family_id not in {"swap-hierarchy", "rollback-hierarchy"}:
        return
    if not {"event_class", "event_count"} <= set(frame.columns):
        raise ValueError(f"{family_id}: missing event hierarchy fields")
    for (_, _, run_index), group in frame.groupby(
        ["experiment", "condition", "run_index"], sort=False
    ):
        counts = dict(zip(group["event_class"], group["event_count"], strict=True))
        if counts != {"first-use-aot": 1, "cached": 49}:
            raise ValueError(f"{family_id}: run {run_index} mixes cold and cached events")


def validate_enhanced_visual_datasets(
    datasets: dict[str, pd.DataFrame],
    *,
    expected_source_sha: str,
    expected_source_tag: str,
    batch_id: str,
    manifest: dict[str, Any] | None = None,
    layout: ResultsLayout | None = None,
) -> None:
    manifest = manifest or load_enhanced_visual_manifest()
    validate_enhanced_visual_manifest(manifest)
    if set(datasets) != set(REQUIRED_FAMILIES):
        missing = sorted(set(REQUIRED_FAMILIES) - set(datasets))
        extra = sorted(set(datasets) - set(REQUIRED_FAMILIES))
        raise ValueError(f"enhanced visual datasets differ: missing={missing}, extra={extra}")
    if _HEX_SHA.fullmatch(expected_source_sha) is None or not expected_source_tag:
        raise ValueError("expected source SHA/tag is invalid")
    definitions = _definitions(manifest)
    aliases = set(CANONICAL_ALIASES)
    for family_id in REQUIRED_FAMILIES:
        definition = definitions[family_id]
        frame = datasets[family_id]
        if frame.empty:
            raise ValueError(f"{family_id}: empty dataset")
        missing = REQUIRED_DATA_COLUMNS - set(frame.columns)
        if missing:
            raise ValueError(f"{family_id}: missing columns {sorted(missing)}")
        if frame[list(REQUIRED_DATA_COLUMNS)].isna().any().any():
            raise ValueError(f"{family_id}: required values contain nulls")
        if not all(
            math.isfinite(value)
            for value in pd.to_numeric(frame["value"], errors="raise")
        ) or not all(
            math.isfinite(value)
            for value in pd.to_numeric(frame["x_value"], errors="raise")
        ):
            raise ValueError(f"{family_id}: plot values must be finite numbers")
        if set(frame["evidence_class"]) != {definition["evidence_class"]}:
            raise ValueError(f"{family_id}: wrong evidence class")
        if frame["thesis_evidence"].map(type).ne(bool).any() or frame[
            "thesis_evidence"
        ].any():
            raise ValueError(f"{family_id}: candidate or diagnostic data labeled final")
        if frame["n30_admitted"].map(type).ne(bool).any() or frame[
            "n30_admitted"
        ].any():
            raise ValueError(f"{family_id}: data admitted to N=30 before selection")
        if frame["source_dirty"].map(type).ne(bool).any() or frame[
            "source_dirty"
        ].any():
            raise ValueError(f"{family_id}: dirty source evidence")
        if set(frame["source_git_sha"]) != {expected_source_sha}:
            raise ValueError(f"{family_id}: wrong source revision")
        if set(frame["source_tag"]) != {expected_source_tag}:
            raise ValueError(f"{family_id}: wrong source tag")
        if set(frame["batch_id"]) != {batch_id}:
            raise ValueError(f"{family_id}: wrong batch identity")
        if set(frame["experiment"].astype(str)) & aliases:
            raise ValueError(f"{family_id}: canonical alias counted as replication")
        if not all(unit in definition["units"] for unit in frame["unit"].unique()):
            raise ValueError(f"{family_id}: undeclared unit")
        if set(frame["sample_unit"]) != {definition["sample_unit"]}:
            raise ValueError(f"{family_id}: sample unit differs from manifest")
        for relative, source_sha256 in frame[
            ["source_relative_path", "source_sha256"]
        ].drop_duplicates().itertuples(index=False):
            if re.fullmatch(r"[0-9a-f]{64}", str(source_sha256)) is None:
                raise ValueError(f"{family_id}: invalid source artifact hash")
            source = _validate_source_path(str(relative), layout)
            if source is not None and _sha256(source) != source_sha256:
                raise ValueError(f"{family_id}: source artifact hash differs")
        counts = frame.groupby(["experiment", "condition"], sort=False)[
            "run_index"
        ].nunique()
        if not counts.eq(definition["independent_n"]).all():
            raise ValueError(f"{family_id}: missing independent N")
        if frame.duplicated(
            ["experiment", "condition", "run_index", "sample_id", "metric"]
        ).any():
            raise ValueError(f"{family_id}: duplicate nested sample identity")
        if family_id == "swap-10ms-timeline":
            if "drain_right_censored" not in frame or frame[
                "drain_right_censored"
            ].map(type).ne(bool).any() or frame["drain_right_censored"].any():
                raise ValueError("swap-10ms-timeline: right-censored drain")
        if family_id == "delivery-probability" and not frame["value"].between(
            0, 1, inclusive="both"
        ).all():
            raise ValueError("delivery-probability: values must be proportions")
        _validate_interval_rows(family_id, frame)
        _validate_fine_swap_rows(family_id, frame)
        _validate_capacity_censoring(family_id, frame)
        _validate_swap_hierarchy(family_id, frame)


def _write_csv(path: Path, frame: pd.DataFrame) -> None:
    columns = sorted(frame.columns)
    frame.sort_values(
        ["experiment", "condition", "run_index", "sample_id", "metric"],
        kind="stable",
    ).to_csv(path, columns=columns, index=False, quoting=csv.QUOTE_MINIMAL, lineterminator="\n")


def _plot(paths: dict[str, Path], family: dict[str, Any], frame: pd.DataFrame) -> None:
    units = list(dict.fromkeys(frame["unit"].astype(str)))
    figure, axes = plt.subplots(len(units), 1, figsize=(9, max(4, 3.3 * len(units))))
    if len(units) == 1:
        axes = [axes]
    for axis, unit in zip(axes, units, strict=True):
        selected = frame.loc[frame["unit"] == unit]
        for metric, group in selected.groupby("metric", sort=True):
            points = group[["x_value", "value"]].astype(float)
            axis.scatter(points["x_value"], points["value"], s=14, alpha=0.28)
            medians = points.groupby("x_value", sort=True)["value"].median()
            axis.plot(
                medians.index,
                medians.values,
                marker="o",
                linewidth=1.8,
                label=str(metric),
            )
        axis.set_xlabel("Declared x value")
        axis.set_ylabel(unit)
        axis.legend()
    figure.suptitle(family["title"])
    figure.text(
        0.01,
        0.01,
        "DIAGNOSTIC N=5 — NOT THESIS EVIDENCE",
        color="#9d2d25",
        fontsize=8,
        weight="bold",
    )
    figure.tight_layout(rect=(0, 0.04, 1, 0.96))
    figure.savefig(
        paths["svg"],
        format="svg",
        metadata={"Date": None, "Creator": "WAFER enhanced visual suite"},
    )
    figure.savefig(
        paths["png"],
        format="png",
        metadata={"Software": "WAFER enhanced visual suite"},
    )
    figure.savefig(
        paths["pdf"],
        format="pdf",
        metadata={
            "CreationDate": None,
            "ModDate": None,
            "Creator": "WAFER enhanced visual suite",
            "Producer": "WAFER enhanced visual suite",
        },
    )
    plt.close(figure)


def _sha256(path: Path) -> str:
    return hashlib.sha256(path.read_bytes()).hexdigest()


def _table(frame: pd.DataFrame) -> str:
    columns = [
        column
        for column in (
            "condition",
            "run_index",
            "metric",
            "value",
            "unit",
            "source_tag",
            "control_generation",
            "source_relative_path",
            "source_sha256",
            "pair_status",
        )
        if column in frame.columns
    ]
    rows = []
    for row in (
        frame.sort_values(columns[:3], kind="stable")
        .head(12)
        .itertuples(index=False)
    ):
        values = row._asdict()
        cells = "".join(
            f"<td>{html.escape(str(values[column]))}</td>" for column in columns
        )
        rows.append(f"<tr>{cells}</tr>")
    headings = "".join(
        f'<th scope="col">{html.escape(column)}</th>' for column in columns
    )
    return f"<table><thead><tr>{headings}</tr></thead><tbody>{''.join(rows)}</tbody></table>"


def _dataframe_table(frame: pd.DataFrame) -> str:
    columns = list(frame.columns)
    headings = "".join(
        f'<th scope="col">{html.escape(str(column))}</th>' for column in columns
    )
    rows = []
    for row in frame.itertuples(index=False):
        values = row._asdict()
        cells = "".join(
            f"<td>{html.escape(str(values[column]))}</td>" for column in columns
        )
        rows.append(f"<tr>{cells}</tr>")
    return f"<table><thead><tr>{headings}</tr></thead><tbody>{''.join(rows)}</tbody></table>"


def _write_family_table(path: Path, family: dict[str, Any], frame: pd.DataFrame) -> None:
    path.write_text(
        "<!doctype html>\n<html lang=\"en\"><head><meta charset=\"utf-8\">"
        f"<title>{html.escape(family['title'])}</title></head><body>"
        f"<h1>{html.escape(family['title'])}</h1>"
        f"<p><strong>Classification:</strong> {html.escape(family['evidence_class'])}</p>"
        f"<p><strong>Sample unit:</strong> {html.escape(family['sample_unit'])}</p>"
        f"<p><strong>Observed N=5 pattern:</strong> "
        f"{html.escape(family['observed_n5_pattern'])}</p>"
        f"<p><strong>Release composition:</strong> "
        f"{html.escape(json.dumps(family.get('release_composition', {}), sort_keys=True))}</p>"
        f"<p><strong>Limits:</strong> {html.escape(family['limits'])}</p>"
        f"{_table(frame)}</body></html>\n",
        encoding="utf-8",
    )


def _report(
    path: Path,
    manifest: dict[str, Any],
    datasets: dict[str, pd.DataFrame],
    derived: Path,
    *,
    title: str = "Enhanced N=5 candidate and diagnostic visual suite",
    introduction: str = "PRE-RESULTS — NOT THESIS EVIDENCE. campaign_started=false; candidate selection remains required.",
    supporting_links: tuple[str, ...] = (),
) -> None:
    cards = []
    relative_derived = Path(os.path.relpath(derived, path.parent)).as_posix()
    for family in manifest["families"]:
        family_id = family["id"]
        base = f"{relative_derived}/{family_id}"
        family_title = html.escape(family["title"])
        fields = html.escape(", ".join(family["source_fields"]))
        units = html.escape(", ".join(family["units"]))
        sample_unit = html.escape(family["sample_unit"])
        how_to_read = html.escape(family["how_to_read"])
        observed = html.escape(family["observed_n5_pattern"])
        limits = html.escape(family["limits"])
        cards.append(
            f"""
<article data-visual-family="{family_id}">
  <h2>{family_title}</h2>
  <a href="{base}.svg"><img src="{base}.svg" alt="{family_title}"></a>
  <p><a href="{base}.csv">CSV</a> · <a href="{base}.png">PNG</a> · <a href="{base}.pdf">PDF</a> · <a href="{base}.html">HTML table</a></p>
  <h3>Source fields</h3><p>{fields}</p>
  <h3>Units</h3><p>{units}</p>
  <h3>Sample unit</h3><p>{sample_unit}; independent N={family['independent_n']}.</p>
  <h3>How to read it</h3><p>{how_to_read}</p>
  <h3>Observed N=5 pattern</h3><p>{observed}</p>
  <h3>Limits</h3><p>{limits}</p>
  <h3>Supporting table</h3>{_table(datasets[family_id])}
</article>"""
        )
    supporting = "".join(
        f'<li><a href="{relative_derived}/{html.escape(name)}.csv">{html.escape(name)} CSV</a> · '
        f'<a href="{relative_derived}/{html.escape(name)}.html">HTML table</a></li>'
        for name in supporting_links
    )
    external_gaps = "".join(
        f"<li>{html.escape(str(item['experiment']))}: "
        f"{html.escape(str(item['status']))} — {html.escape(str(item['reason']))}</li>"
        for item in manifest.get("external_gaps", [])
    )
    path.write_text(
        """<!doctype html>
<html lang="en">
<head><meta charset="utf-8"><title>WAFER enhanced N=5 visuals</title></head>
<body><main>
"""
        + f"<h1>{html.escape(title)}</h1>\n<p><strong>{html.escape(introduction)}</strong></p>\n"
        + (f"<h2>External gaps</h2><ul>{external_gaps}</ul>\n" if external_gaps else "")
        + (f"<h2>Release sensitivity</h2><ul>{supporting}</ul>\n" if supporting else "")
        + "\n".join(cards)
        + "\n</main></body></html>\n",
        encoding="utf-8",
    )


def _render_validated_visual_suite(
    datasets: dict[str, pd.DataFrame],
    layout: ResultsLayout,
    *,
    batch_id: str,
    manifest: dict[str, Any],
    artifact_metadata: dict[str, Any],
    report_title: str,
    report_introduction: str,
    supporting_tables: dict[str, pd.DataFrame] | None = None,
) -> tuple[Path, Path]:
    derived = layout.analysis_path("derived", "enhanced-n5", batch_id)
    reports = layout.analysis_path("reports", "enhanced-n5", batch_id)
    if derived.exists() or reports.exists():
        raise FileExistsError("refusing to overwrite enhanced analysis output")
    derived.mkdir(parents=True)
    reports.mkdir(parents=True)
    plt.rcParams["svg.hashsalt"] = "wafer-enhanced-visual-suite-v1"
    artifacts = []
    for family in manifest["families"]:
        family_id = family["id"]
        paths = {
            kind: derived / f"{family_id}.{kind}"
            for kind in ("svg", "png", "pdf", "html")
        }
        csv_path = derived / f"{family_id}.csv"
        _write_csv(csv_path, datasets[family_id])
        _plot(paths, family, datasets[family_id])
        _write_family_table(paths["html"], family, datasets[family_id])
        artifacts.append(
            {
                "id": family_id,
                "csv": csv_path.name,
                "csv_sha256": _sha256(csv_path),
                "svg": paths["svg"].name,
                "svg_sha256": _sha256(paths["svg"]),
                "png": paths["png"].name,
                "png_sha256": _sha256(paths["png"]),
                "pdf": paths["pdf"].name,
                "pdf_sha256": _sha256(paths["pdf"]),
                "html": paths["html"].name,
                "html_sha256": _sha256(paths["html"]),
                "row_count": len(datasets[family_id]),
                "source_file_count": datasets[family_id][
                    "source_relative_path"
                ].nunique(),
            }
        )
    supporting_artifacts = []
    for name, frame in sorted((supporting_tables or {}).items()):
        csv_path = derived / f"{name}.csv"
        html_path = derived / f"{name}.html"
        frame.to_csv(csv_path, index=False, lineterminator="\n")
        html_path.write_text(
            "<!doctype html>\n<html lang=\"en\"><head><meta charset=\"utf-8\">"
            f"<title>{html.escape(name)}</title></head><body><h1>{html.escape(name)}</h1>"
            f"{_dataframe_table(frame)}</body></html>\n",
            encoding="utf-8",
        )
        supporting_artifacts.append(
            {
                "id": name,
                "csv": csv_path.name,
                "csv_sha256": _sha256(csv_path),
                "html": html_path.name,
                "html_sha256": _sha256(html_path),
                "row_count": len(frame),
            }
        )
    observations = None
    if manifest.get("status") == "completed":
        observations = derived / "observations.json"
        observations.write_text(
            json.dumps(manifest, indent=2, sort_keys=True) + "\n", encoding="utf-8"
        )
    artifact_manifest = {
        "schema_version": 1,
        "classification": manifest["classification"],
        "thesis_evidence": False,
        "n30_admitted": False,
        "campaign_started": False,
        "batch_id": batch_id,
        **artifact_metadata,
        "visual_contract_sha256": hashlib.sha256(
            json.dumps(manifest, sort_keys=True, separators=(",", ":")).encode()
        ).hexdigest(),
        "artifact_count": len(artifacts),
        "supporting_artifact_count": len(supporting_artifacts),
        "artifacts": artifacts,
        "supporting_artifacts": supporting_artifacts,
        **(
            {
                "observations": observations.name,
                "observations_sha256": _sha256(observations),
            }
            if observations is not None
            else {}
        ),
    }
    manifest_path = derived / "artifact-manifest.json"
    manifest_path.write_text(
        json.dumps(artifact_manifest, indent=2, sort_keys=True) + "\n",
        encoding="utf-8",
    )
    _report(
        reports / "index.html",
        manifest,
        datasets,
        derived,
        title=report_title,
        introduction=report_introduction,
        supporting_links=tuple(item["id"] for item in supporting_artifacts),
    )
    validate_enhanced_visual_artifacts(derived, reports, manifest)
    return derived, reports


def render_enhanced_visual_suite(
    datasets: dict[str, pd.DataFrame],
    layout: ResultsLayout,
    *,
    batch_id: str,
    expected_source_sha: str,
    expected_source_tag: str,
    manifest: dict[str, Any] | None = None,
) -> tuple[Path, Path]:
    manifest = manifest or load_enhanced_visual_manifest()
    if not layout.explicit:
        raise ValueError("enhanced suite requires an explicit results volume")
    layout.validate()
    validate_enhanced_visual_datasets(
        datasets,
        expected_source_sha=expected_source_sha,
        expected_source_tag=expected_source_tag,
        batch_id=batch_id,
        manifest=manifest,
        layout=layout,
    )
    return _render_validated_visual_suite(
        datasets,
        layout,
        batch_id=batch_id,
        manifest=manifest,
        artifact_metadata={
            "source_git_sha": expected_source_sha,
            "source_tag": expected_source_tag,
        },
        report_title="Enhanced N=5 candidate and diagnostic visual suite",
        report_introduction=(
            "PRE-RESULTS — NOT THESIS EVIDENCE. "
            "campaign_started=false; candidate selection remains required."
        ),
    )


def render_completed_n5_visual_suite(
    datasets: dict[str, pd.DataFrame],
    layout: ResultsLayout,
    *,
    batch_id: str,
    manifest: dict[str, Any],
    source_manifest_sha256: str,
    source_composite_sha256: str,
    analyzer_git_sha: str,
    analyzer_tag: str,
    release_composition: dict[str, int],
    supporting_tables: dict[str, pd.DataFrame] | None = None,
) -> tuple[Path, Path]:
    if not layout.explicit:
        raise ValueError("enhanced suite requires an explicit results volume")
    layout.validate()
    validate_completed_visual_manifest(manifest)
    return _render_validated_visual_suite(
        datasets,
        layout,
        batch_id=batch_id,
        manifest=manifest,
        artifact_metadata={
            "source_manifest_sha256": source_manifest_sha256,
            "source_composite_sha256": source_composite_sha256,
            "analyzer_git_sha": analyzer_git_sha,
            "analyzer_tag": analyzer_tag,
            "release_composition": release_composition,
        },
        report_title="DIAGNOSTIC N=5 REVIEW — NOT THESIS EVIDENCE",
        report_introduction=(
            "Completed diagnostic observations. candidate selection remains required; "
            "campaign_started=false; n30_admitted=false."
        ),
        supporting_tables=supporting_tables,
    )


def validate_enhanced_visual_artifacts(
    derived: Path, reports: Path, manifest: dict[str, Any] | None = None
) -> None:
    manifest = manifest or load_enhanced_visual_manifest()
    artifact_manifest = json.loads(
        (derived / "artifact-manifest.json").read_text(encoding="utf-8")
    )
    if (
        artifact_manifest.get("artifact_count") != len(REQUIRED_FAMILIES)
        or artifact_manifest.get("thesis_evidence") is not False
        or artifact_manifest.get("n30_admitted") is not False
        or artifact_manifest.get("campaign_started") is not False
        or tuple(item.get("id") for item in artifact_manifest.get("artifacts", []))
        != REQUIRED_FAMILIES
    ):
        raise ValueError("enhanced artifact manifest is invalid")
    expected_files = {"artifact-manifest.json"}
    if manifest.get("status") == "completed":
        observations = derived / str(artifact_manifest.get("observations", ""))
        if not observations.is_file() or _sha256(observations) != artifact_manifest.get(
            "observations_sha256"
        ):
            raise ValueError("completed observations hash differs")
        if json.loads(observations.read_text(encoding="utf-8")) != manifest:
            raise ValueError("completed observations differ from visual manifest")
        expected_files.add(observations.name)
    for item in artifact_manifest["artifacts"]:
        for kind in ("csv", "svg", "png", "pdf", "html"):
            expected_files.add(str(item[kind]))
            path = derived / item[kind]
            if not path.is_file() or not path.stat().st_size:
                raise ValueError(f"{item['id']}: missing {kind}")
            if _sha256(path) != item[f"{kind}_sha256"]:
                raise ValueError(f"{item['id']}: {kind} hash differs")
    supporting_artifacts = artifact_manifest.get("supporting_artifacts", [])
    if artifact_manifest.get("supporting_artifact_count") != len(supporting_artifacts):
        raise ValueError("enhanced supporting artifact count differs")
    for item in supporting_artifacts:
        for kind in ("csv", "html"):
            expected_files.add(str(item[kind]))
            path = derived / item[kind]
            if not path.is_file() or _sha256(path) != item[f"{kind}_sha256"]:
                raise ValueError(f"{item['id']}: supporting {kind} hash differs")
    observed_files = {path.name for path in derived.iterdir() if path.is_file()}
    if observed_files != expected_files:
        raise ValueError("enhanced artifact file set differs from manifest")
    report = reports / "index.html"
    if {path.name for path in reports.iterdir() if path.is_file()} != {"index.html"}:
        raise ValueError("enhanced report file set differs")
    parser = _ReportParser()
    parser.feed(report.read_text(encoding="utf-8"))
    if tuple(parser.cards) != REQUIRED_FAMILIES:
        raise ValueError("enhanced HTML cards differ from visual contract")
    for heading in (
        "Source fields",
        "Units",
        "Sample unit",
        "How to read it",
        "Observed N=5 pattern",
        "Limits",
        "Supporting table",
    ):
        if parser.headings.count(heading) != len(REQUIRED_FAMILIES):
            raise ValueError(f"enhanced HTML lacks adjacent {heading} sections")
    for link in parser.links:
        target = (report.parent / link).resolve()
        if not target.is_file():
            raise ValueError(f"enhanced HTML local link is broken: {link}")
    if manifest.get("status") == "completed":
        for field in (
            "source_manifest_sha256",
            "source_composite_sha256",
            "analyzer_git_sha",
        ):
            expected_length = 40 if field == "analyzer_git_sha" else 64
            if re.fullmatch(rf"[0-9a-f]{{{expected_length}}}", str(artifact_manifest.get(field, ""))) is None:
                raise ValueError(f"completed artifact manifest has invalid {field}")
        if artifact_manifest.get("analyzer_tag") != "rpi5-final-rc-v17" or not isinstance(
            artifact_manifest.get("release_composition"), dict
        ):
            raise ValueError("completed artifact manifest has invalid provenance")
        text = "\n".join(
            (derived / f"{family_id}.html").read_text(encoding="utf-8")
            for family_id in REQUIRED_FAMILIES
        )
        if "PRE-RESULTS" in text.upper() or "PENDING" in text.upper():
            raise ValueError("completed family artifacts contain stale pre-results text")
