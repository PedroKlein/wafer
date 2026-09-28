"""Table export for the analysis notebooks."""

from pathlib import Path

import pandas as pd


def save_table(df: pd.DataFrame, name: str, directory: str | Path) -> tuple[Path, Path]:
    """Write one reproducible table as CSV data and LaTeX presentation."""
    output = Path(directory)
    output.mkdir(parents=True, exist_ok=True)
    csv_path = output / f"{name}.csv"
    tex_path = output / f"{name}.tex"
    df.to_csv(csv_path, index=False)
    tex_path.write_text(df.to_latex(index=False, escape=True))
    return csv_path, tex_path

