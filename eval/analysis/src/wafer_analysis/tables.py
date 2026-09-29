"""Table export for the analysis notebooks."""

import numbers
from pathlib import Path

import numpy as np
import pandas as pd

NOTE_COLUMNS = ("units", "estimator", "uncertainty")
DESCRIPTIVE_COLUMNS = (*NOTE_COLUMNS, "threshold", "claim_boundary", "thesis_evidence", "reason")
SOURCE_LINE = "Source: the author (2026)."


def save_table(
    df: pd.DataFrame,
    name: str,
    directory: str | Path,
    *,
    caption: str | None = None,
    columns: list[str] | None = None,
) -> tuple[Path, Path]:
    """Write every column as CSV data and the numeric body as a LaTeX table.

    The LaTeX body keeps ``columns``, by default every column except the
    descriptive text columns, and folds each ``*ci95_low*``/``*ci95_high*`` pair into the estimate
    column before it as ``estimate [low, high]`` when the names show the pair
    belongs to that estimate.

    With a caption the LaTeX is a ``table`` float labelled ``tab:<name>`` whose
    note carries the units, estimator and uncertainty text and the source line.
    """
    output = Path(directory)
    output.mkdir(parents=True, exist_ok=True)
    csv_path = output / f"{name}.csv"
    tex_path = output / f"{name}.tex"
    df.to_csv(csv_path, index=False)
    selected = columns or [column for column in df.columns if column not in DESCRIPTIVE_COLUMNS]
    body = _with_intervals(df[selected])
    body.columns = [column.replace("_", " ") for column in body.columns]
    tabular = body.to_latex(index=False, escape=True, na_rep="--", formatters={column: _format for column in body})
    if caption is None:
        tex_path.write_text(tabular)
        return csv_path, tex_path
    notes = [
        f"{column.capitalize()}: {_latex_escape(str(df[column].dropna().iloc[0]))}."
        for column in NOTE_COLUMNS
        if column in df and df[column].notna().any()
    ]
    tex_path.write_text(
        "\\begin{table}[htbp]\n\\centering\n"
        f"\\caption{{{_latex_escape(caption)}}}\n\\label{{tab:{name}}}\n"
        f"{tabular}"
        f"\\par\\smallskip\\footnotesize {' '.join([*notes, SOURCE_LINE])}\n"
        "\\end{table}\n"
    )
    return csv_path, tex_path


def _with_intervals(body: pd.DataFrame) -> pd.DataFrame:
    body = body.copy()
    columns = list(body.columns)
    for index, low in enumerate(columns[1:-1], start=1):
        high = low.replace("ci95_low", "ci95_high")
        if "ci95_low" not in low or columns[index + 1] != high:
            continue
        estimate = columns[index - 1]
        if not _interval_belongs_to(low, estimate):
            continue
        body[estimate] = [
            "--" if pd.isna(value) else f"{_format(value)} [{_format(lower)}, {_format(upper)}]"
            for value, lower, upper in zip(body[estimate], body[low], body[high], strict=True)
        ]
        body = body.drop(columns=[low, high])
    return body


def _interval_belongs_to(low: str, estimate: str) -> bool:
    prefix = low[: low.index("ci95_low")].strip("_")
    tokens = estimate.split("_")
    if prefix:
        return all(token in tokens for token in prefix.split("_"))
    return len(tokens) == 2 and tokens[0] == "median"


def _format(value: object) -> str:
    if isinstance(value, bool | np.bool_):
        return "yes" if value else "no"
    if isinstance(value, numbers.Real):
        if abs(value) >= 100 or float(value).is_integer():
            return f"{value:,.0f}"
        return f"{value:.3g}"
    return str(value)


def _latex_escape(text: str) -> str:
    replacements = {"\\": r"\textbackslash{}", "&": r"\&", "%": r"\%", "$": r"\$", "#": r"\#", "_": r"\_", "{": r"\{", "}": r"\}", "~": r"\textasciitilde{}", "^": r"\textasciicircum{}"}
    return "".join(replacements.get(character, character) for character in text)
