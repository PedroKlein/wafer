"""Load HdrHistogram logs and CSV result files."""

from pathlib import Path

import numpy as np
import pandas as pd


def load_hdr_log(path: str | Path) -> np.ndarray:
    """Load an HdrHistogram interval log and return latency values in nanoseconds.
    
    Parses the standard HdrHistogram log format (lines starting with #
    are comments, data lines contain: start_time, end_time, max, histogram_data).
    """
    # Simplified: load from CSV export (one value per line in ns)
    values = []
    with open(path) as f:
        for line in f:
            line = line.strip()
            if not line or line.startswith("#") or line.startswith("\""):
                continue
            try:
                values.append(float(line))
            except ValueError:
                continue
    return np.array(values)


def load_csv_results(path: str | Path) -> pd.DataFrame:
    """Load a CSV results file (throughput, memory, per-node metrics)."""
    return pd.read_csv(path)


def load_per_node_metrics(path: str | Path) -> pd.DataFrame:
    """Load per-node metrics CSV: node_id, messages, failures, process_ns, swap_count."""
    return pd.read_csv(path)
