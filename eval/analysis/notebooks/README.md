# Evaluation analysis notebooks

The notebooks consume one explicitly identified result batch. They never select the newest directory implicitly.

## Notebook reference

| Notebook | Experiment | Output |
|---|---|---|
| `00-warmup-validation.ipynb` | E-Val-1 | 50 ms measurement honesty gate |
| `01-latency-cdf.ipynb` | E-Perf-2 | Matched target-load latency percentiles |
| `02-per-hop-overhead.ipynb` | E-Perf-4 | Payload-size boundary cost |
| `03-memory-scaling.ipynb` | E-Perf-6 | Pipeline-depth RSS |
| `04-cross-arch.ipynb` | E-Perf-5 | ARM64/x86 WAFER-to-native ratio, or an explicit no-claim result |
| `04-metering-overhead.ipynb` | E-Perf-7 | Auxiliary metering view |
| `04b-depth-scaling.ipynb` | E-Perf-8 | Auxiliary in-process depth view |
| `05-hotswap-timeline.ipynb` | E-Swap | Internal HTTP duration and sink-observed output gap |
| `06-fault-injection.ipynb` | E-Iso-4, E-Iso-7 | Epoch recovery and independent branch-A measurements |
| `07-metering-decomp.ipynb` | E-Perf-7 | Fuel and epoch metering decomposition |
| `08-depth-scaling.ipynb` | E-Perf-3, E-Perf-8 | MQTT-bookended and in-process depth scaling |
| `09-backpressure.ipynb` | E-Backpressure | Bounded-channel occupancy and flow rates |
| `09-saturation.ipynb` | E-Perf-10 | Offered-load sweep; E-Perf-1 remains target-load evidence |
| `10-aot-startup.ipynb` | E-Perf-9 | Filesystem and compiled-component cache state by startup phase |
| `10-summary-stats.ipynb` | Focused set | Artifact availability inventory without repeated headline claims |

All output is inline. Figures and tables label the independent run count, units, evidence status, and uncertainty status. Diagnostic focused-pilot data use `thesis_evidence=false` and descriptive uncertainty. Power values, when present, are labelled **Raspberry Pi 5 PMIC internal-rail proxy** rather than total board power.

Missing and failed conditions remain `PENDING` with a null value. They are not converted to zero or omitted.

## Focused-pilot artifact inventory

| Follow-up question | Notebook | Primary artifact |
|---|---|---|
| eKuiper latency tail | `09-saturation.ipynb` | `rate-sweep.json` |
| target load versus saturation | `09-saturation.ipynb` | `rate-sweep-summary.json` |
| branch-A throughput and latency | `06-fault-injection.ipynb` | `branch-isolation.json` |
| epoch recovery | `06-fault-injection.ipynb` | `containment.json` |
| startup cache state | `10-aot-startup.ipynb` | `startup.json` |
| bounded queue pressure | `09-backpressure.ipynb` | `backpressure.json` |
| internal and sink-observed hot-swap timing | `05-hotswap-timeline.ipynb` | `hotswap-analysis.json` |

The machine-readable inventory is `wafer_analysis.focused.artifact_inventory`.

## Visual review checklist

The complete and missing-leaf synthetic executions are reviewed for these properties:

- [x] READY tables show N, units, evidence status, and descriptive uncertainty.
- [x] Missing and failed leaves show `PENDING` with null values.
- [x] Target-load and saturation labels remain distinct.
- [x] Branch-A measurements do not include branch-B populations.
- [x] HTTP duration and sink-observed hot-swap gaps use separate columns.
- [x] Startup output distinguishes filesystem state from compiled-cache state.
- [x] Queue output separates offered, accepted, processed, and drained rates.
- [x] Power wording says PMIC internal-rail proxy, not total-board power.

`test_notebook_execution.py` executes every notebook against both fixture states; `test_focused.py` checks labels and inventory coverage.

## Run an explicit canonical batch

Install the analysis environment, export the batch identifier once, then execute a notebook:

```bash
cd eval/analysis
uv sync
export WAFER_EVAL_BATCH_ID=<batch-id>
uv run jupyter nbconvert --execute --to notebook --output-dir /tmp \
  notebooks/09-saturation.ipynb
```

`WAFER_EVAL_BATCH_ID` resolves `eval/results/<experiment>/rpi5-<batch-id>/`. Canonical resolution verifies Raspberry Pi 5 provenance, a clean tagged source, zero throttling, passed completion receipts, and one source SHA.

## Run an explicit diagnostic directory

Each notebook accepts experiment-specific directory variables such as `E_PERF_10_DIR`, `E_ISO_7_DIR`, and `E_SWAP_DIR`:

```bash
cd eval/analysis
E_PERF_10_DIR=/absolute/path/to/e-perf-10 \
  uv run jupyter nbconvert --execute --to notebook --output-dir /tmp \
  notebooks/09-saturation.ipynb
```

`resolve_result_batch()` also accepts `diagnostic_path` directly:

```python
from wafer_analysis.paths import resolve_result_batch

result_dir = resolve_result_batch(
    "e-perf-10",
    diagnostic_path="/absolute/path/to/e-perf-10",
)
```

The path must exist. When no diagnostic path is supplied, `WAFER_EVAL_BATCH_ID` is mandatory.

See [`eval/RESULT-CONTRACT.md`](../../RESULT-CONTRACT.md) for result-leaf schemas and provenance requirements.
