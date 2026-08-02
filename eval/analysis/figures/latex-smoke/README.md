# T9 LaTeX embed dry-run

Verifies that PDF figures produced by
`wafer_analysis.plots.save_figure()` embed into a plain LaTeX document
via `\includegraphics{...}` with no font substitution warnings.

## Build

```bash
cd eval/analysis/figures/latex-smoke
pdflatex -interaction=nonstopmode embed.tex
# grep for font warnings (should return nothing):
grep -Ei 'font shape|substituted|font.*undefined|missing.*font' embed.log
```

## AC (T9, thesis-hardening plan)

> AC: One thesis-embed dry-run: pick figure 2 (per-hop overhead),
> embed in a minimal LaTeX doc, verify no font substitution warnings.

Passing evidence (2026-08-02, first run):

- Compiler: pdflatex from TeXLive 2025
- Font trace in log: `lmbx10 lmbx12 lmr10 lmtt10` (Latin Modern +
  matplotlib's TrueType embed) — no substitution.
- Output: `embed.pdf` (98 KB, single page).
- `grep -Ei 'font shape|substituted|font.*undefined|missing.*font' embed.log`: NONE FOUND

Build artefacts (`embed.aux`, `embed.log`, `embed.pdf`) are `.gitignore`d;
`embed.tex` is checked in for reproducibility. Any future
matplotlib-toolchain change should be sanity-checked by re-running this
smoke build.
