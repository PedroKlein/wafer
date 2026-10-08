# eval/analysis

The `wafer_analysis` package and the notebooks that turn a result batch into
thesis figures and tables. It is a `uv` project: `uv sync`, then
`uv run pytest -q` (or `mise run test-eval` from the repo root).

| Path | Role |
|---|---|
| `src/wafer_analysis/attempts.py`, `backpressure.py`, `rollback.py`, `results_layout.py` | Attempt classification, result validators and layout helpers. The campaign runner imports all four and the verifier imports `attempts.py` and `results_layout.py`, so they are part of the measurement path. |
| `src/wafer_analysis/capacity_brackets.py` | Derives a host's E-Perf-10 bracket rates from its capacity scout summary and reads them back from `batch.json`. The campaign runner and the canonical analysis gate both use it. |
| `src/wafer_analysis/canonical.py` | Final N=30 tables (target latency, metering, capacity knee, backpressure, swaps). |
| `src/wafer_analysis/verdicts.py` | Reads the declared thresholds from `eval/canonical-matrix.json` and turns one-sided bootstrap bounds or exact counts into `PASS`, `FAIL`, `INCONCLUSIVE` or `PENDING`. |
| `src/wafer_analysis/paths.py`, `focused.py`, `plots.py`, `tables.py`, `stats.py` | Batch resolution, evidence labels, thesis matplotlib style, table export, bootstrap CI and Cliff's delta. |
| `src/wafer_analysis/host.py` | Per-core CPU use, the attempts table and batch progress. |
| `src/wafer_analysis/power.py`, top-level `canonical_power.py` | Per-host power summary and its figure (Pi 5 PMIC internal-rail proxy, Jetson INA3221 rail proxy, x86 RAPL package power), labelled from each leaf's `power-boundary.json`. |
| `notebooks/` | One notebook per experiment family; see `notebooks/README.md`. |
| `test_*.py` | Unit tests plus `test_notebook_execution.py`, which executes every notebook against fixtures. |

`mise run //eval:figures` executes the notebooks with
`WAFER_ANALYSIS_OUTPUT_DIR` set and writes PDF, PNG, CSV and LaTeX outputs
under `figures/` (ignored by git; the thesis keeps its own copies).
