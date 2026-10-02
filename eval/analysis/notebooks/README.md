# Evaluation analysis notebooks

The notebooks consume one explicitly identified result batch. They never select the newest directory implicitly.

## Notebook reference

| Notebook | Experiment | Output |
|---|---|---|
| `00-warmup-validation.ipynb` | E-Val-1 | 50 ms measurement honesty gate |
| `01-latency-cdf.ipynb` | E-Perf-2 | Matched target-load latency percentiles |
| `02-per-hop-overhead.ipynb` | E-Perf-4 | Paired WAFER-minus-native service time by payload size, in-process path only |
| `03-memory-scaling.ipynb` | E-Perf-6 | Pipeline-depth RSS |
| `04-cross-arch.ipynb` | E-Perf-5 | ARM64/x86 WAFER-to-native ratio, or an explicit no-claim result; per-host paired WAFER/native contrast of run p50 |
| `05-hotswap-timeline.ipynb` | E-Swap | Internal HTTP duration and sink-observed output gap |
| `06-fault-injection.ipynb` | E-Iso-4, E-Iso-7 | Epoch recovery and independent branch-A measurements |
| `07-metering-decomp.ipynb` | E-Perf-7 | Fuel and epoch metering decomposition |
| `08-depth-scaling.ipynb` | E-Perf-3, E-Perf-8 | MQTT-bookended and in-process depth scaling |
| `09-backpressure.ipynb` | E-Backpressure | Bounded-channel occupancy and flow rates |
| `09-saturation.ipynb` | E-Perf-10, E-Perf-1 | Offered-load sweep; E-Perf-1 target-load delivery, paired WAFER minus native contrast and replication concordance |
| `10-aot-startup.ipynb` | E-Perf-9 | Filesystem and compiled-component cache state by startup phase |
| `10-summary-stats.ipynb` | E-Density-1; whole batch | Release component sizes; units, attempts, clean passes, system outcomes, infrastructure failures, retries and missing units per experiment, system and condition; wall time and peak temperature per experiment |

Tables remain inline. Final figures are also written as PDF and PNG, and final tables as CSV and LaTeX, when `WAFER_ANALYSIS_OUTPUT_DIR` is set. Every output labels the independent run count, units, estimator, evidence status, uncertainty, and claim boundary. Explicit diagnostic inputs are always forced to `thesis_evidence=false` with descriptive uncertainty, regardless of labels inside historical metadata. Power values, when present, are labelled **Raspberry Pi 5 PMIC internal-rail proxy** rather than total board power.

Missing and failed diagnostic conditions remain `PENDING` with a null value. They are not converted to zero or omitted. Canonical mode is different: wrong-host, dirty, mixed-SHA, throttled, failed, malformed, incomplete, or unapproved input raises an error instead of rendering a partial result.

## Verdicts

Every threshold a notebook compares against comes from `verdict_rules` in `eval/canonical-matrix.json`, read through `wafer_analysis.verdicts.declared_thresholds()`; no notebook or table builder keeps its own copy, and plot reference lines read the same values. A criterion with a threshold is `PASS` only when its one-sided 95% bound on the favourable side meets the threshold, `FAIL` only when the one-sided 95% bound on the other side misses it, and `INCONCLUSIVE` otherwise. The bounds are the two ends of a two-sided 90% percentile bootstrap interval that resamples runs, or run pairs for the E-Perf-1 p95 ratio and E-Perf-4, whose conditions share one randomised block per run index. A criterion with no population to bound is `PENDING`. Zero-tolerance counts (loss, duplication, escapes, runs the system under test stopped) are compared exactly and never get an interval. E-Perf-10 keeps its tested-rate bracket (`PASS`, `FAIL` or `CENSORED`) and E-Val-1 keeps its every-run band. No significance test is run: verdicts come from bootstrap bounds only.

Saved verdict tables carry `<prefix>_verdict`, `<prefix>_estimate` (the point estimate of the bounded statistic), `<prefix>_threshold`, `<prefix>_flips_at` (the bound the threshold has to cross to change the verdict) and `<prefix>_ci_half_width` for each bounded criterion, and `verdict` where a row combines criteria. `eval/RESULT-CONTRACT.md` lists every threshold, its rule and its columns.

## Paired contrasts

`09-saturation.ipynb` saves `e-perf-1-wafer-native-contrast`: WAFER and native run p95 and p50 paired by run index, with the median over pairs of WAFER minus native, its bootstrap 95% CI over run pairs and half-width, the ratio of medians with its CI, the paired SD and the minimum detectable difference `(z(0.975) + z(0.80)) * paired SD / sqrt(N pairs)`. `04-cross-arch.ipynb` saves `e-perf-5-wafer-native-contrast`, the same paired contrast of run p50 with one row per host, labelled with its host tag. A canonical run reads every host's approved E-Perf-5 batch from `eval/final-batches.json`; a diagnostic run reads `E_PERF_5_RPI_DIR`, `E_PERF_5_JETSON_DIR` and `E_PERF_5_X86_DIR`, and setting any of them makes every host diagnostic. Runs the system under test stopped early are left out with their partner runs and counted in `runs_stopped_early`. Both are descriptive and carry no verdict; the E-Perf-5 rows make no cross-architecture claim. `eval/RESULT-CONTRACT.md` lists their columns.

## Replication concordance

`09-saturation.ipynb` also compares each replication host's E-Perf-1 verdicts with the Raspberry Pi 5 verdicts under `replication_concordance` in `eval/canonical-matrix.json` and saves `e-perf-1-replication-concordance`. Each criterion and row is `same-verdict`, `same-direction` (different verdicts, point estimates on the same side of the threshold), `opposite-direction` or `not-estimable` (missing, `PENDING` or without a point estimate on either host). It compares the p95 ratio, pooled loss, achieved ratio and duplicate count; an exact count's point estimate is the count itself. Combined verdicts are not classified, but every criterion they combine is. The table never changes a Raspberry Pi 5 verdict. A canonical run reads every host's approved E-Perf-1 batch from `eval/final-batches.json`; a diagnostic run compares `E_PERF_1_DIR` with `E_PERF_1_JETSON_DIR` and `E_PERF_1_X86_DIR` when they are set, and those leaves must carry `throughput.csv` and `sequence.csv` target-load evidence.

Percentile summaries are never presented as an empirical CDF.

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
