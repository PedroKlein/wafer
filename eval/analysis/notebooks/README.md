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

## Enhanced N=5 visual suite

`eval/analysis/enhanced-visual-manifest.json` defines twelve pre-results chart/table
families for the v5 candidates and diagnostics: interval latency/throughput,
capacity knee, delivery-good frequency, payload knee, extended depth latency/RSS,
the actual-t0 10 ms swap window, independent swap and rollback hierarchies, the
thermal/load ladder, USB integrity, eKuiper tail association, and PMIC-proxy
efficiency. `wafer_analysis.enhanced_visuals` validates normalized source tables
and emits one deterministic SVG and CSV per family plus a local-link HTML report.

Every source row is bound to the explicit batch, clean source SHA, and release
tag. The validator rejects missing independent N, aliases counted as replication,
uncensored capacity rows, mixed first-use/cached event populations,
right-censored E-Swap-4 drain evidence, undeclared units, raw-tree output, and any
candidate or diagnostic row labeled as final evidence. Until the expanded N=5
run exists, each explanation states `PENDING`; fixture rendering proves the
contract and layout only and does not create measured results.

## Completed expanded N=5 review

`wafer_analysis.expanded_n5` is a separate result-time adapter for the immutable
mixed v10-v13 diagnostic population. It does not relax canonical single-release
APIs. The adapter requires the complete raw SHA-256 manifest, source-host seal,
composite index, and macOS `handoff-verified` receipt; it never globs for a
newest or merely passed attempt. Every normalized row retains its selected
result key, release tag/SHA, control generation, source-relative path, and
artifact SHA-256. B00 thermal/USB rows come only from the prerequisite record in
the composite.

After signed v14 is mounted with the same USB at `/Volumes/WAF_RESULTS`, run:

```bash
cd eval/analysis
uv run python -m wafer_analysis.expanded_n5 \
  --results-root /Volumes/WAF_RESULTS \
  --manifest /Volumes/WAF_RESULTS/manifests/expanded-n5.sha256 \
  --source-seal /Volumes/WAF_RESULTS/manifests/n5-batches/<batch>/source-seal.json \
  --composite /Volumes/WAF_RESULTS/manifests/n5-batches/<batch>/composite-index.json \
  --handoff-receipt /Volumes/WAF_RESULTS/manifests/storage-qualification/<id>/handoff-macos.json \
  --analyzer-git-sha <signed-v14-wafer-sha> \
  --analyzer-tag rpi5-final-rc-v14
```

The command first rehashes the complete raw manifest. It writes only below
`derived/enhanced-n5/<batch>/` and `reports/enhanced-n5/<batch>/`, with CSV,
SVG, PNG, PDF, and HTML supporting tables for all twelve families plus a
hash-bound result-time `observations.json`. The report is headed `DIAGNOSTIC N=5
REVIEW — NOT THESIS EVIDENCE`. Family observations contain no historical
`PRE-RESULTS` or `PENDING` text; genuine external gaps such as unmatched x86
E-Perf-5 remain explicit in the report-level external-gap section.

Matched arms are paired only when both leaves have the same evidence release.
Cross-release arms are labeled `release-confounded`, remain visible as separate
arms, and are excluded from paired differences. Release-stratified sensitivity
CSV/HTML tables are emitted alongside the family artifacts. The analyzer's v14
tag/SHA is recorded only in derived artifact provenance; raw evidence keeps its
actual v10-v13 lineage.

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

Install the analysis environment, provide the human approval receipt, export the batch identifier once, then execute a notebook:

```bash
cd eval/analysis
uv sync
export WAFER_FULL_RUN_APPROVAL=../../.plans/rpi5-final-experiment-readiness/full-run-approval.json
export WAFER_EVAL_BATCH_ID=<batch-id>
export WAFER_ANALYSIS_OUTPUT_DIR=figures/final-<batch-id>
uv run jupyter nbconvert --execute --to notebook --output-dir executed \
  notebooks/09-saturation.ipynb
```

`WAFER_EVAL_BATCH_ID` resolves `eval/results/<experiment>/rpi5-<batch-id>/`. Canonical resolution verifies the approval decision and batch identity, Raspberry Pi 5 provenance, final-evidence labeling, a clean tagged source, zero throttling, passed completion receipts, one source SHA, every required artifact, and the exact condition/run population declared by `eval/canonical-matrix.json`. It never selects the latest batch implicitly.

The analysis gate consumes a minimal approval subset: `schema_version=1`, `decision=APPROVE`, `batch_id`, `wafer_git_sha`, and `canonical_matrix_sha256`. The T14 launch receipt is a strict superset and additionally records its timestamp, release tag, tcc-doc SHA, schedule hash, binary/plugin receipts, targeted-pilot ID, runtime/storage estimates, and `campaign_started=false`. Explicit diagnostic paths never consume or satisfy the approval gate.

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
