# Evaluation Analysis Notebooks

> **All outputs are shakedown-macos quality — NOT thesis-grade canonical results.**

## Canonical notebook index (plan P7.1)

| # | File | Experiment | Figure/Table |
|---|------|-----------|-------------|
| 00 | `00-warmup-validation.ipynb` | E-Val-1 | Figure 0: honesty gate |
| 01 | `01-latency-cdf.ipynb` | E-Perf-2 | Figure 1: latency CDF |
| 02 | `02-per-hop-overhead.ipynb` | E-Perf-4 | Figure 2: per-hop × payload |
| 03 | `03-memory-scaling.ipynb` | E-Perf-6 | Figure 3: RSS scaling |
| 04 | `04-cross-arch.ipynb` | E-Perf-5 | Figure 4: cross-arch ratio (PENDING Pi) |
| 05 | `05-hotswap-timeline.ipynb` | E-Swap-1..6 | Figures 5–8: hot-swap analysis |
| 06 | `06-fault-injection.ipynb` | E-Iso-1..8 | Figure 9, Table 3: attack containment |
| 07 | `07-metering-decomp.ipynb` | E-Perf-7 | Figure 10, Table 5: metering overhead |
| 08 | `08-depth-scaling.ipynb` | E-Perf-3 vs 8 | Figure 11, Table 6: depth scaling |
| 09 | `09-saturation.ipynb` | E-Perf-1 | Figure 12, Table 1: throughput comparison |
| 10 | `10-summary-stats.ipynb` | All | Table 7: startup; cross-RQ summary |

## Auxiliary notebooks (additional analysis, not in canonical list)

| File | Experiment | Notes |
|------|-----------|-------|
| `04-metering-overhead.ipynb` | E-Perf-7 | Original implementation; `07-metering-decomp.ipynb` is the canonical version with full statistical pipeline |
| `04b-depth-scaling.ipynb` | E-Perf-8 | Early single-experiment view; `08-depth-scaling.ipynb` has the dual-plot (E-Perf-3 vs E-Perf-8) canonical version |
| `09-backpressure.ipynb` | E-Backpressure | Burst backpressure validation; supplementary to the main throughput analysis |
| `10-aot-startup.ipynb` | E-Perf-9 | Detailed cold/warm startup; `10-summary-stats.ipynb` references it for Table 7 |

## Running

```bash
# Execute all canonical notebooks headless
cd eval/analysis
uv run jupyter execute notebooks/00-warmup-validation.ipynb
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

All notebooks import `find_latest_shakedown` from `eval/analysis/utils.py`
to resolve the newest result directory for a given experiment.  This
eliminates hardcoded timestamps that rot as new shakedown runs land.

```python
import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent.parent if '__file__' in dir() else Path.cwd().parent))
from utils import find_latest_shakedown

# Automatically picks the newest shakedown result (lex sort on ISO-8601 timestamps)
RESULT_DIR = find_latest_shakedown('e-val-1')

# Pin a specific run when reproducibility of a figure is needed:
RESULT_DIR = find_latest_shakedown('e-val-1', pinned='eval/results/e-val-1/shakedown-macos-2026-07-22T16-19-29Z')

# Override via environment variable (useful in CI / scripted reruns):
import os
RESULT_DIR = find_latest_shakedown('e-perf-4', pinned=os.environ.get('SHAKEDOWN_DIR'))
```

The `pinned` parameter accepts an absolute path or a repo-relative path.
When `None` (the default), the helper globs and returns the lexicographically
last match.

## Data contract

Each notebook resolves its input data from `eval/results/<experiment>/shakedown-macos-*/`.
The result directories are gitignored; notebooks must handle missing data gracefully
(raise `FileNotFoundError` with a clear message about which experiment needs to run).

See `eval/RESULT-CONTRACT.md` for the full result directory manifest specification.
