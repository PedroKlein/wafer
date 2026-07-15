"""LaTeX table generation from analysis results."""

import pandas as pd


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
