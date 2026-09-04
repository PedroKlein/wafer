"""LaTeX table generation from analysis results."""

from pathlib import Path

import pandas as pd

HOTSWAP_DURATION_FIELDS = (
    "compile_ns",
    "instantiate_ns",
    "signal_ns",
    "ack_ns",
    "convergence_ns",
    "http_total_ns",
    "sink_observed_output_gap_ns",
)
HOTSWAP_INTERPRETATION = (
    "Internal swap phases, HTTP request duration, and sink-observed output gaps are "
    "separate measurements. A smaller sink-observed output gap does not imply a faster "
    "internal swap because queued output can mask internal disruption."
)


def hotswap_timeline_table(evidence: dict) -> tuple[pd.DataFrame, str]:
    source_leaf = evidence.get("measurement_source_leaf")
    if not isinstance(source_leaf, str) or not source_leaf:
        raise ValueError("hot-swap evidence requires measurement_source_leaf")

    rows = []
    for event in evidence.get("events", []):
        missing = [field for field in HOTSWAP_DURATION_FIELDS if field not in event]
        if missing:
            raise ValueError(
                f"hot-swap event missing raw nanosecond fields: {', '.join(missing)}"
            )
        if any(key.endswith("_ms") for key in event):
            raise ValueError("hot-swap raw evidence must use nanosecond field names")
        row = {
            "experiment": str(evidence.get("experiment", "")),
            "condition": str(evidence.get("condition", "")),
            "event_index": int(event["event_index"]),
        }
        row.update(
            {
                field.removesuffix("_ns") + "_ms": int(event[field]) / 1_000_000
                for field in HOTSWAP_DURATION_FIELDS
            }
        )
        row["measurement_source_leaf"] = source_leaf
        rows.append(row)

    columns = [
        "experiment",
        "condition",
        "event_index",
        *(field.removesuffix("_ns") + "_ms" for field in HOTSWAP_DURATION_FIELDS),
        "measurement_source_leaf",
    ]
    return pd.DataFrame(rows, columns=columns), HOTSWAP_INTERPRETATION


def save_table(df: pd.DataFrame, name: str, directory: str | Path) -> tuple[Path, Path]:
    """Write one reproducible table as CSV data and LaTeX presentation."""
    output = Path(directory)
    output.mkdir(parents=True, exist_ok=True)
    csv_path = output / f"{name}.csv"
    tex_path = output / f"{name}.tex"
    df.to_csv(csv_path, index=False)
    tex_path.write_text(df.to_latex(index=False, escape=True))
    return csv_path, tex_path


def results_to_latex(
    df: pd.DataFrame,
    caption: str,
    label: str,
    columns: list[str] | None = None,
) -> str:
    """Convert a DataFrame to a LaTeX table string.

    Args:
        df: Data to render.
        caption: Table caption.
        label: LaTeX label for cross-referencing.
        columns: Subset of columns to include (None = all).

    Returns:
        Complete LaTeX table environment string.
    """
    if columns:
        df = df[columns]

    latex = df.to_latex(index=False, escape=True)

    return (
        f"\\begin{{table}}[htbp]\n"
        f"\\centering\n"
        f"\\caption{{{caption}}}\n"
        f"\\label{{{label}}}\n"
        f"{latex}"
        f"\\end{{table}}\n"
    )
