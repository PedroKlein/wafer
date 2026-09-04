"""WAFER thesis analysis utilities."""

from .canonical import (
    FINAL_VISUAL_MANIFEST,
    capacity_tables,
    metering_table,
    swap3_table,
    swap4_table,
    target_latency_table,
    validate_visual_manifest,
)
from .focused import (
    artifact_inventory,
    evidence_label,
    passed_artifacts,
    pending_record,
    percentile_rows,
)
from .hdr_loader import load_csv_results, load_hdr_log
from .paths import (
    analysis_evidence_status,
    find_canonical_batch,
    require_cross_architecture,
    resolve_analysis_batch,
    resolve_result_batch,
)
from .plots import save_figure, setup_thesis_style
from .stats import bootstrap_ci, cliffs_delta, mann_whitney_u, shapiro_wilk
from .tables import results_to_latex

__all__ = [
    "FINAL_VISUAL_MANIFEST",
    "analysis_evidence_status",
    "artifact_inventory",
    "bootstrap_ci",
    "capacity_tables",
    "cliffs_delta",
    "evidence_label",
    "find_canonical_batch",
    "load_csv_results",
    "load_hdr_log",
    "mann_whitney_u",
    "metering_table",
    "passed_artifacts",
    "pending_record",
    "percentile_rows",
    "require_cross_architecture",
    "resolve_analysis_batch",
    "resolve_result_batch",
    "results_to_latex",
    "save_figure",
    "setup_thesis_style",
    "shapiro_wilk",
    "swap3_table",
    "swap4_table",
    "target_latency_table",
    "validate_visual_manifest",
]
