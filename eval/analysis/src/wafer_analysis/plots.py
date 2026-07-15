"""Thesis-quality plot styling and figure generation."""

from pathlib import Path

import matplotlib.pyplot as plt
import seaborn as sns

THESIS_STYLE = {
    "font.family": "serif",
    "font.size": 10,
    "axes.titlesize": 11,
    "axes.labelsize": 10,
    "xtick.labelsize": 9,
    "ytick.labelsize": 9,
    "legend.fontsize": 9,
    "figure.figsize": (6, 4),
    "figure.dpi": 300,
    "savefig.dpi": 300,
    "savefig.bbox": "tight",
    "axes.grid": True,
    "grid.alpha": 0.3,
    "grid.linestyle": "--",
}


def setup_thesis_style() -> None:
    """Apply thesis-quality matplotlib styling."""
    plt.rcParams.update(THESIS_STYLE)
    sns.set_palette("colorblind")


def save_figure(fig: plt.Figure, name: str, directory: str = "../figures") -> Path:
    """Save figure as PDF and PNG for thesis inclusion."""
    out_dir = Path(directory)
    out_dir.mkdir(parents=True, exist_ok=True)
    
    pdf_path = out_dir / f"{name}.pdf"
    png_path = out_dir / f"{name}.png"
    
    fig.savefig(pdf_path, format="pdf")
    fig.savefig(png_path, format="png")
    
    return pdf_path
