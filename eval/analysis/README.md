# eval/analysis

The `wafer_analysis` package and the notebooks that turn a result batch into
thesis figures and tables. It is a `uv` project: `uv sync`, then
`uv run pytest -q` (or `mise run test-eval` from the repo root).

| Path | Role |
|---|---|
| `src/wafer_analysis/backpressure.py`, `rollback.py`, `results_layout.py` | Result validators and layout helpers. The campaign runner imports all three and the verifier imports `results_layout.py`, so they are part of the measurement path. |
| `src/wafer_analysis/canonical.py` | Final N=30 tables (target latency, metering, capacity knee, backpressure, swaps). |
| `src/wafer_analysis/paths.py`, `focused.py`, `plots.py`, `tables.py`, `stats.py` | Batch resolution, evidence labels, thesis matplotlib style, table export, bootstrap CI and Cliff's delta. |
| `src/wafer_analysis/power.py`, `canonical_power.py` | PMIC rail proxy power summary and its figure. |
| `notebooks/` | One notebook per experiment family; see `notebooks/README.md`. |
| `test_*.py` | Unit tests plus `test_notebook_execution.py`, which executes every notebook against fixtures. |

`mise run //eval:figures` executes the notebooks with
`WAFER_ANALYSIS_OUTPUT_DIR` set and writes PDF, PNG, CSV and LaTeX outputs
under `figures/` (ignored by git; the thesis keeps its own copies).
