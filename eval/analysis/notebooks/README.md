# Evaluation Analysis Notebooks

Canonical thesis exports require an explicit `WAFER_EVAL_BATCH_ID`. Without it, the legacy shakedown lookup remains available only for development checks.

## Canonical notebook index (plan P7.1 + T11 traceability)

Each row maps a canonical notebook to its RFC-008 experiment id, the
research question it exercises, the thesis figure/table it produces,
the input result directory it reads, and the figure output paths it
writes. The table is the reverse index for `docs/benchmarks/rq-summary.md`
— every RQ metric there links back to the notebook cell that produces it.

| # | File | Experiment | RQ | Figure/Table | Input data | Output figure(s) |
|---|------|-----------|-----|--------------|------------|-----------------|
| 00 | [`00-warmup-validation.ipynb`](00-warmup-validation.ipynb) | E-Val-1 | Methodology | Figure 0: honesty gate | `eval/results/e-val-1/shakedown-macos-*/` | inline (methodology only) |
| 01 | [`01-latency-cdf.ipynb`](01-latency-cdf.ipynb) | E-Perf-2 | RQ1 | Figure 1: latency CDF | `eval/results/e-perf-2/shakedown-macos-*/` | `figures/e-perf-2/latency_cdf.{png,pdf}` |
| 02 | [`02-per-hop-overhead.ipynb`](02-per-hop-overhead.ipynb) | E-Perf-4 | RQ1 | Figure 2: per-hop × payload | `eval/results/e-perf-4/shakedown-macos-*/` | `figures/e-perf-4/per_hop_overhead.{png,pdf}` |
| 03 | [`03-memory-scaling.ipynb`](03-memory-scaling.ipynb) | E-Perf-6 | RQ1 | Figure 3: RSS scaling | `eval/results/e-perf-6/shakedown-macos-*/` | `figures/e-perf-6/rss_scaling.{png,pdf}` |
| 04 | [`04-cross-arch.ipynb`](04-cross-arch.ipynb) | E-Perf-5 | RQ1 | Figure 4: cross-arch ratio | `eval/results/e-perf-5/shakedown-{macos,pi}-*/` | `figures/e-perf-5/cross_arch.{png,pdf}` (PENDING Pi) |
| 05 | [`05-hotswap-timeline.ipynb`](05-hotswap-timeline.ipynb) | E-Swap-1..6 | RQ3 | Figures 5–8: hot-swap analysis | `eval/results/e-swap-{1,2,4,5,6}/shakedown-macos-*/` | `figures/e-swap/{pause,zeroloss,burst,rollback,phases}.{png,pdf}` |
| 06 | [`06-fault-injection.ipynb`](06-fault-injection.ipynb) | E-Iso-1..8 | RQ2 | Figure 9, Table 3: attack containment | `eval/results/e-iso-{1..8}/shakedown-macos-*/` | `figures/e-iso/containment.{png,pdf}` |
| 07 | [`07-metering-decomp.ipynb`](07-metering-decomp.ipynb) | E-Perf-7 | RQ1 | Figure 10, Table 5: metering overhead | `eval/results/e-perf-7/shakedown-macos-*/` | `figures/e-perf-7/metering_decomp.{png,pdf}` |
| 08 | [`08-depth-scaling.ipynb`](08-depth-scaling.ipynb) | E-Perf-3 + E-Perf-8 | RQ1 | Figure 11, Table 6: depth scaling | `eval/results/e-perf-{3,8}/shakedown-macos-*/` | `figures/e-perf-8/depth_scaling.{png,pdf}` |
| 09 | [`09-saturation.ipynb`](09-saturation.ipynb) | E-Perf-1 | RQ1 | Figure 12, Table 1: throughput comparison | `eval/results/e-perf-1/shakedown-macos-*/` | `figures/e-perf-1/throughput.{png,pdf}` |
| 10 | [`10-summary-stats.ipynb`](10-summary-stats.ipynb) | All | All | Table 7: startup; cross-RQ summary | `eval/results/**/shakedown-macos-*/` | `figures/summary/startup_stats.{png,pdf}` |

**Legend:**
- **RQ**: research question(s) the notebook informs (RQ1 = performance,
  RQ2 = fault containment, RQ3 = hot-swap).
- **Input data**: canonical result-dir glob resolved by
  `wafer_analysis.paths.find_latest_shakedown()`. Notebooks that consume
  data from multiple experiments list every input.
- **Output figure(s)**: relative to `eval/analysis/figures/`. Both PNG
  (120 DPI, screen) and PDF (vector, thesis embed) are produced by
  `wafer_analysis.plots.save_figure`; see T9 (thesis-hardening) for the
  PDF/font pipeline. Notebooks marked “inline” ship figures inside the
  notebook cell for reviewer inspection only — they do not export.

## Auxiliary notebooks (additional analysis, not in canonical list)

| File | Experiment | Notes |
|------|-----------|-------|
| `04-metering-overhead.ipynb` | E-Perf-7 | Original implementation; `07-metering-decomp.ipynb` is the canonical version with full statistical pipeline |
| `04b-depth-scaling.ipynb` | E-Perf-8 | Early single-experiment view; `08-depth-scaling.ipynb` has the dual-plot (E-Perf-3 vs E-Perf-8) canonical version |
| `09-backpressure.ipynb` | E-Backpressure | Burst backpressure validation; supplementary to the main throughput analysis |
| `10-aot-startup.ipynb` | E-Perf-9 | Detailed cold/warm startup; `10-summary-stats.ipynb` references it for Table 7 |

## Running

> **Prerequisite:** the `wafer_analysis` package must be importable. The
> canonical setup is `uv sync` from `eval/analysis/` — this installs the
> package in editable mode via the `[tool.hatch.build.targets.wheel]` entry
> in `pyproject.toml`, so every subsequent `uv run …` command sees
> `from wafer_analysis.paths import find_latest_shakedown`. Outside `uv`,
> `pip install -e eval/analysis` gives the same result; or set
> `PYTHONPATH=$(pwd)/eval/analysis/src` for a one-shot invocation.

```bash
# One-time setup (skip if you already `uv sync`'d)
cd eval/analysis
uv sync

# Execute all canonical notebooks headless
# Execute canonical notebooks against one explicit Pi 5 batch
WAFER_EVAL_BATCH_ID=<batch-id> uv run jupyter execute notebooks/00-warmup-validation.ipynb
uv run jupyter execute notebooks/01-latency-cdf.ipynb
uv run jupyter execute notebooks/02-per-hop-overhead.ipynb
uv run jupyter execute notebooks/03-memory-scaling.ipynb
uv run jupyter execute notebooks/04-cross-arch.ipynb
uv run jupyter execute notebooks/05-hotswap-timeline.ipynb
uv run jupyter execute notebooks/06-fault-injection.ipynb
uv run jupyter execute notebooks/07-metering-decomp.ipynb
uv run jupyter execute notebooks/08-depth-scaling.ipynb
uv run jupyter execute notebooks/09-saturation.ipynb
uv run jupyter execute notebooks/10-summary-stats.ipynb
```

## Path discovery helper

All notebooks import `find_latest_shakedown` from the `wafer_analysis`
package (`eval/analysis/src/wafer_analysis/paths.py`) to resolve the
newest result directory for a given experiment. This eliminates
hardcoded timestamps that rot as new shakedown runs land.

```python
from wafer_analysis.paths import find_latest_shakedown

# Automatically picks the newest shakedown result (lex sort on ISO-8601 timestamps)
RESULT_DIR = find_latest_shakedown('e-val-1')

# Pin a specific run when reproducibility of a figure is needed:
RESULT_DIR = find_latest_shakedown('e-val-1', pinned='eval/results/e-val-1/shakedown-macos-2026-07-22T16-19-29Z')

# Override via environment variable (useful in CI / scripted reruns):
import os
RESULT_DIR = find_latest_shakedown('e-perf-4', pinned=os.environ.get('SHAKEDOWN_DIR'))
```

The helper also resolves an explicit canonical batch when `WAFER_EVAL_BATCH_ID` is set. Canonical resolution validates `rpi5` provenance, clean tagged source, zero throttling, passed run receipts, and a single source SHA before returning the path.

The helper is also re-exported at the package root:
`from wafer_analysis import find_latest_shakedown`.

The `pinned` parameter accepts an absolute path or a repo-relative path.
When `None` (the default), the helper globs and returns the lexicographically
last match.

## Data contract

Each notebook resolves its input data from `eval/results/<experiment>/shakedown-macos-*/`.
The result directories are gitignored; notebooks must handle missing data gracefully
(raise `FileNotFoundError` with a clear message about which experiment needs to run).

See `eval/RESULT-CONTRACT.md` for the full result directory manifest specification.
