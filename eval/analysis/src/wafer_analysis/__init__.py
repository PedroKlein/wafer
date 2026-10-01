"""WAFER thesis analysis utilities."""

from .canonical import (
    FINAL_VISUAL_MANIFEST,
    candidate_capacity_table,
    candidate_depth_table,
    candidate_payload_table,
    candidate_swap_tables,
    backpressure_table,
    capacity_tables,
    ekuiper_profile_tables,
    failed_replacement_summary,
    failed_replacement_table,
    metering_table,
    swap3_table,
    swap4_table,
    target_latency_table,
    validate_visual_manifest,
)
from .focused import (
    evidence_label,
    admitted_artifacts,
    admitted_runs,
    pending_record,
    percentile_rows,
)
from .paths import (
    analysis_evidence_status,
    find_canonical_batch,
    require_cross_architecture,
    resolve_analysis_output,
    resolve_analysis_batch,
    resolve_result_batch,
)
from .plots import save_figure, setup_thesis_style
from .stats import bootstrap_ci, cliffs_delta

__all__ = [
    "FINAL_VISUAL_MANIFEST",
    "analysis_evidence_status",
    "bootstrap_ci",
    "candidate_capacity_table",
    "candidate_depth_table",
    "candidate_payload_table",
    "candidate_swap_tables",
    "backpressure_table",
    "capacity_tables",
    "cliffs_delta",
    "ekuiper_profile_tables",
    "failed_replacement_summary",
    "failed_replacement_table",
    "evidence_label",
    "find_canonical_batch",
    "metering_table",
    "admitted_artifacts",
    "admitted_runs",
    "pending_record",
    "percentile_rows",
    "require_cross_architecture",
    "resolve_analysis_batch",
    "resolve_analysis_output",
    "resolve_result_batch",
    "save_figure",
    "setup_thesis_style",
    "swap3_table",
    "swap4_table",
    "target_latency_table",
    "validate_visual_manifest",
]
