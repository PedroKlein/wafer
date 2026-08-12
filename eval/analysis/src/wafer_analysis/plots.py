"""Thesis-quality plot styling and figure generation.

Provides two entry points used across canonical notebooks:

- ``setup_thesis_style()`` — apply the thesis-grade matplotlib rcParams.
  Fonts are pinned to matplotlib's bundled Computer-Modern-compatible
  serif (STIX/DejaVu Serif math) so the setup is portable across hosts
  without needing a system-installed LaTeX distribution. PDF/PS output
  uses ``fonttype = 42`` so glyphs embed as TrueType, which is what
  every LaTeX PDF pipeline (pdflatex, LuaLaTeX, XeLaTeX) accepts
  without substitution warnings.
- ``save_figure(fig, name)`` — write PNG (120 DPI, screen) + PDF
  (vector, no rasterisation) side-by-side into the canonical figures
  directory. Every canonical notebook that produces a thesis figure
  should end with this call.

T9 (thesis-hardening plan, 2026-08-02) pins the fonttype/family choice
and introduces the dual-format output so PDF figures survive a plain
``\\includegraphics{name}`` in the thesis LaTeX source with no font
substitution warnings.
"""

from pathlib import Path

import matplotlib.pyplot as plt
import seaborn as sns

THESIS_STYLE = {
    # Serif family; matplotlib picks DejaVu Serif (bundled) when Computer
    # Modern is not installed, which renders body glyphs at Computer-Modern
    # weight without needing a system LaTeX install.
    "font.family": "serif",
    "font.serif": [
        "Computer Modern Roman",  # if TeX is installed
        "CMU Serif",              # cm-super / cm-unicode
        "Latin Modern Roman",     # LuaLaTeX default
        "DejaVu Serif",           # matplotlib bundled fallback
        "serif",
    ],
    # Math font: 'cm' uses matplotlib's built-in Computer-Modern-compatible
    # STIX math rendering. This is what makes \alpha, subscripts, etc. render
    # in a serif that matches body text without a TeX toolchain.
    "mathtext.fontset": "cm",
    "font.size": 10,
    "axes.titlesize": 11,
    "axes.labelsize": 10,
    "xtick.labelsize": 9,
    "ytick.labelsize": 9,
    "legend.fontsize": 9,
    "figure.figsize": (6, 4),
    "figure.dpi": 120,
    "savefig.dpi": 120,
    "savefig.bbox": "tight",
    "axes.grid": True,
    "grid.alpha": 0.3,
    "grid.linestyle": "--",
    # Force TrueType glyph embed in PDF/PS output. `fonttype = 42` is
    # the AC-required value (T9): pdflatex/LuaLaTeX/XeLaTeX accept it
    # without any font substitution warnings. `fonttype = 3` (default)
    # produces Type-3 fonts that trip warnings on most journal LaTeX
    # templates.
    "pdf.fonttype": 42,
    "ps.fonttype": 42,
    # Never rasterise vector primitives inside the PDF output. The AC
    # says 'PDF (vector, no rasterisation)'. Individual figures can
    # still opt-in via `ax.set_rasterization_zorder(0)` when a large
    # scatter/heatmap would blow up file size.
    "pdf.compression": 6,
}

# Canonical system colour mapping used across all comparison figures.
# Deck palette: rust-orange for WAFER, muted grey for Native, violet for eKuiper.
# Import this dict in any notebook that plots per-system series so colours
# stay consistent across figures.
SYSTEM_COLORS = {
    'WAFER': '#c2410c',
    'Native': '#6b6156',
    'eKuiper': '#6d28d9',
}

# Canonical output directory relative to the notebook. Notebooks live at
# eval/analysis/notebooks/*.ipynb; the figures dir is a sibling of
# notebooks/ so "../figures/e-perf-4/per_hop_overhead.pdf" resolves cleanly.
DEFAULT_FIGURES_DIR = "../figures"


def setup_thesis_style() -> None:
    """Apply thesis-quality matplotlib styling in-place on ``plt.rcParams``.

    Idempotent. Call once near the top of every canonical notebook.
    """
    plt.rcParams.update(THESIS_STYLE)
    sns.set_palette("colorblind")


def save_figure(
    fig: plt.Figure,
    name: str,
    directory: str = DEFAULT_FIGURES_DIR,
) -> Path:
    """Save ``fig`` as both PDF (vector, thesis embed) and PNG (screen).

    Parameters
    ----------
    fig:
        Matplotlib Figure to persist.
    name:
        Filename stem — the function appends `.pdf` and `.png`.
        Nested paths (e.g. ``"e-perf-4/per_hop_overhead"``) are honoured;
        subdirectories are created as needed.
    directory:
        Base directory; defaults to ``../figures`` (sibling of
        ``notebooks/`` when called from a notebook).

    Returns
    -------
    Path
        Absolute path to the PDF (the thesis embed target). The PNG
        sits next to it.
    """
    out_dir = Path(directory)
    out_dir.mkdir(parents=True, exist_ok=True)
    stem_path = out_dir / name
    stem_path.parent.mkdir(parents=True, exist_ok=True)

    pdf_path = stem_path.with_suffix(".pdf")
    png_path = stem_path.with_suffix(".png")

    # PDF: vector, TrueType-embedded (via rcParams above).
    fig.savefig(pdf_path, format="pdf")
    # PNG: 120 DPI for screen review; upgrade to 300 DPI by passing
    # `dpi=300` from the caller if a paper venue requires it.
    fig.savefig(png_path, format="png", dpi=120)

    return pdf_path

