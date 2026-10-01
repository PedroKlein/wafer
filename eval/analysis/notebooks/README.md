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
| `05-hotswap-timeline.ipynb` | E-Swap | Internal HTTP duration and sink-observed output gap |
| `06-fault-injection.ipynb` | E-Iso-4, E-Iso-7 | Epoch recovery and independent branch-A measurements |
| `07-metering-decomp.ipynb` | E-Perf-7 | Fuel and epoch metering decomposition |
| `08-depth-scaling.ipynb` | E-Perf-3, E-Perf-8 | MQTT-bookended and in-process depth scaling |
| `09-backpressure.ipynb` | E-Backpressure | Bounded-channel occupancy and flow rates |
| `09-saturation.ipynb` | E-Perf-10 | Offered-load sweep; E-Perf-1 remains target-load evidence |
| `10-aot-startup.ipynb` | E-Perf-9 | Filesystem and compiled-component cache state by startup phase |
| `10-summary-stats.ipynb` | E-Density-1; whole batch | Release component sizes; units, attempts, clean passes, system outcomes, infrastructure failures, retries and missing units per experiment, system and condition; wall time and peak temperature per experiment |

Tables remain inline. Final figures are also written as PDF and PNG, and final tables as CSV and LaTeX, when `WAFER_ANALYSIS_OUTPUT_DIR` is set. Every output labels the independent run count, units, estimator, evidence status, uncertainty, and claim boundary. Explicit diagnostic inputs are always forced to `thesis_evidence=false` with descriptive uncertainty, regardless of labels inside historical metadata. Power values, when present, are labelled **Raspberry Pi 5 PMIC internal-rail proxy** rather than total board power.

Missing and failed diagnostic conditions remain `PENDING` with a null value. They are not converted to zero or omitted. Canonical mode is different: wrong-host, dirty, mixed-SHA, throttled, failed, malformed, incomplete, or unapproved input raises an error instead of rendering a partial result.

## Final visual manifest

`wafer_analysis.canonical.FINAL_VISUAL_MANIFEST` is the machine-readable manifest. It keeps these metric groups separate:

- E-Perf-1/2 run-level target-load latency;
- E-Perf-7 metering effect;
- E-Perf-10 offered versus achieved rate, pooled loss, p99 latency, delivery ceiling, normalized p99 knee, and MQTT support-path limitation;
- E-Swap internal phases and sink-observed gaps;
- E-Swap-3 event-aligned dip, action duration, recovery, and sequence integrity;
- E-Swap-4 one event from each independent burst run, plus separately reported source-origin `[0,120s)` primary and `[120s,130s)` drain completion evidence.

Percentile summaries are never presented as an empirical CDF.

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

`test_notebook_execution.py` executes every notebook against both fixture states; `test_focused.py` checks labels.

## Run an explicit canonical batch

Approve the finished batch first (`mise run approve-batch`, see the [runbook](../../../docs/eval/pi5-experiment-runbook.md#approve-the-finished-batch)). Then install the analysis environment, export the batch identifier once, and execute a notebook:

```bash
cd eval/analysis
uv sync
export WAFER_EVAL_BATCH_ID=<batch-id>
export WAFER_ANALYSIS_OUTPUT_DIR=figures/final-<batch-id>
uv run jupyter nbconvert --execute --to notebook --output-dir executed \
  notebooks/09-saturation.ipynb
```

`WAFER_EVAL_BATCH_ID` resolves `eval/results/<experiment>/rpi5-<batch-id>/`. Canonical resolution verifies the batch against its entry in `eval/final-batches.json`, Raspberry Pi 5 provenance, final-evidence labeling, a clean source, zero throttling, one admitted completion receipt per unit (a clean pass or a system outcome), one source SHA, every required artifact, and the exact condition/run population declared by `eval/canonical-matrix.json`. It never selects the latest batch implicitly.

`WAFER_EVAL_BATCH_ID=jetson-<batch-id>` or `x86-<batch-id>` selects that host's batch instead. Each host has its own entry in `eval/final-batches.json`, and every leaf must carry that host's `host_tag`. A batch is never validated against another host's entry, and `require_cross_architecture` checks the Pi and x86 E-Perf-5 batches each against their own. Power reports take their label from each leaf's `power-boundary.json` and refuse to mix hosts or measurements in one figure.

`mise run approve-batch -- --batch-id <id> --host <host>` writes the entry after it has checked that the finished batch is complete, clean, and from one commit. The gate then requires that the batch directory is `<host>-<batch_id>` from the entry, that the ledger `batch.json` has the entry's `wafer_git_sha` and `canonical_matrix_sha256`, that the current `eval/canonical-matrix.json` has that hash, and that the SHA-256 of the ledger's `raw.sha256` equals `raw_manifest_sha256`. It does not hash the raw files again; `sha256sum -c` on `raw.sha256` does that. A missing entry fails with the `approve-batch` command to run. Explicit diagnostic paths never consume or satisfy the approval gate.

## Run an explicit diagnostic directory

Each notebook accepts experiment-specific directory variables such as `E_PERF_10_DIR` and `E_ISO_7_DIR`. The hot-swap notebook accepts `E_SWAP_1_DIR`, `E_SWAP_2_DIR`, `E_SWAP_4_DIR`, and `E_SWAP_6_DIR`; `E_SWAP_DIR` remains an alias for `E_SWAP_1_DIR`.

```bash
cd eval/analysis
E_PERF_10_DIR=../results/e-perf-10/rpi5-<diagnostic-batch-id> \
  uv run jupyter nbconvert --execute --to notebook --output-dir executed \
  notebooks/09-saturation.ipynb
```

`resolve_result_batch()` also accepts `diagnostic_path` directly:

```python
from wafer_analysis.paths import resolve_result_batch

result_dir = resolve_result_batch(
    "e-perf-10",
    diagnostic_path="../results/e-perf-10/rpi5-<diagnostic-batch-id>",
)
```

The path must exist. When no diagnostic path is supplied, `WAFER_EVAL_BATCH_ID` is mandatory.

See [`eval/RESULT-CONTRACT.md`](../../RESULT-CONTRACT.md) for result-leaf schemas and provenance requirements.
