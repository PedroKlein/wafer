"""WAFER thesis analysis utilities."""

from .stats import mann_whitney_u, bootstrap_ci, cliffs_delta, shapiro_wilk
from .plots import setup_thesis_style, save_figure
from .hdr_loader import load_hdr_log, load_csv_results
from .paths import find_canonical_batch, find_latest_shakedown, require_cross_architecture
from .tables import results_to_latex

__all__ = [
    "mann_whitney_u",
    "bootstrap_ci",
    "cliffs_delta",
    "shapiro_wilk",
    "setup_thesis_style",
    "save_figure",
    "load_hdr_log",
    "load_csv_results",
    "find_latest_shakedown",
    "find_canonical_batch",
    "require_cross_architecture",
    "results_to_latex",
]
