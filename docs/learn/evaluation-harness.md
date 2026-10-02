# From experiment definition to evidence

> **Documentation type:** Tutorial
>
> **Prerequisites:** [Learning guide orientation](README.md), basic JSON and Python reading, and the evaluation terminology in `eval/RESULT-CONTRACT.md`.

## Purpose

Trace an evaluation claim from its machine-readable experiment definition through execution, result validation, selected physical evidence, derived datasets, and final evidence classification. The goal is to keep execution count, observation count, and thesis status separate.

## Prerequisites

Read this against a current checkout of `main`. Do not open or quote raw result values to complete the walkthrough. Paths, schemas, validators, and tests are enough to understand the evidence boundary.

## Flow

```mermaid
flowchart TD
    A[canonical-matrix.json definition] --> B[canonical_runner.py schedule]
    B --> C[run-experiment.sh execution]
    C --> D[RESULT-CONTRACT artifacts]
    D --> E[verify-result-contract.py]
    E --> F[selected physical leaves and aliases]
    F --> H[analysis notebooks: figures and tables]
    H --> I{evidence classification}
    I -->|current rehearsal| J[diagnostic and non-poolable]
    I -->|future admitted campaign| K[final N equals 30]
```

1. `eval/canonical-matrix.json` defines experiment IDs, condition grids, repetitions, sample units, required outputs, ordering, metering, evidence class, and admission flags. Each experiment in the `enhanced_candidate` block is N=5 candidate or diagnostic work and sets `thesis_evidence=false` and `n30_admitted=false`.
2. `eval/scripts/lib/canonical_runner.py` turns definitions into deterministic `RunItem` schedules. It dispatches the appropriate execution path, writes progress and summaries, post-processes outputs, and calls `verify_result` for each physical leaf.
3. `eval/scripts/run-experiment.sh` handles one run: configuration, runtime and load-generator processes, bounded collection, shutdown, metadata, and result placement. It delegates fresh local result-directory creation to `eval/scripts/collect-results.sh`, which never overwrites an existing directory.
4. `eval/RESULT-CONTRACT.md` assigns each artifact to its producer and states which experiments require it. `verify-result-contract.py` and experiment-specific checks reject missing, malformed, inconsistent, or wrongly classified evidence.
5. Shared experiment IDs are aliases to one selected physical leaf. An alias receipt preserves identity; it does not copy evidence or create another replicate.
6. The notebooks under `eval/analysis/notebooks/` resolve one explicit batch through `wafer_analysis.paths.resolve_analysis_batch` and render figures and tables that carry run counts, units, estimator, and evidence class.

## Rust

The runtime emits typed JSON and CSV artifacts, but the campaign controller and analysis are Python. `RunItem` is a frozen dataclass carrying one physical run identity. `ResultsLayout` confines raw, manifest, derived, and report paths; analysis reads selected raw files and writes outside the raw tree.

Counts need type-like discipline even though Python tables are dynamic. A complete host run is an independent sample. Messages, aliases, events, buckets, and one-second intervals are nested observations or alternate views. More rows improve within-run detail; they do not increase independent N.

## Design

Evidence classification is part of the data contract, not a caption added later. The completed expanded N=5 rehearsal is diagnostic, descriptive, and non-poolable with final or earlier campaigns. Its records remain `thesis_evidence=false` and outside N=30 admission.

Final N=30 remains PENDING because the frozen final campaign has not been admitted as completed evidence. E-Perf-5 remains PENDING until its matching x86 Linux block exists. PMIC readings remain an internal-rail proxy, not total input power.

Capacity classification is support-aware. When the MQTT loopback support cell is delivery-bad, affected SUT cells at that rate and above are support-confounded. These cells set neither bound of a delivery ceiling, even if a SUT leaf contains measurements. A nested interval, bucket, message, or event cannot remove that censoring.

Do not quote old desktop or Raspberry Pi 4 shakedown values as current or final evidence. Historical layouts remain readable for provenance, not promotion.

## Status boundaries

**Current implementation:** The matrix, runners, result verifier, storage layout, analysis notebooks, and tests encode distinct raw, alias, derived, and evidence-class boundaries. Current accepted N=5 material can support descriptive patterns only.

**Intended design:** A future fully admitted final campaign uses 30 independent runs for its defined estimands and preserves the same artifact and provenance discipline.

**Known drift:** Some matrix entries describe the frozen final design with `thesis_evidence=true`; that declaration does not prove execution or admission. Read it together with campaign status, terminal receipts, and the external-gap rules. In particular, E-Perf-5's final definition remains incomplete until matched x86 execution exists.

## Evidence

- **Source:** [`eval/canonical-matrix.json`](../../eval/canonical-matrix.json) | symbols: `"enhanced_candidate"`, `"thesis_evidence": false`
- **Source:** [`eval/RESULT-CONTRACT.md`](../../eval/RESULT-CONTRACT.md) | symbols: `Intervals and events are nested observations`, `E-Perf-5 remains PENDING`
- **Source:** [`eval/scripts/run-experiment.sh`](../../eval/scripts/run-experiment.sh) | symbols: `Usage: run-experiment.sh`, `eval/RESULT-CONTRACT.md`, `eval/scripts/collect-results.sh`
- **Source:** [`eval/scripts/collect-results.sh`](../../eval/scripts/collect-results.sh) | symbols: `never overwrites an existing directory`, `mkdir -p "$target"`
- **Source:** [`eval/scripts/lib/canonical_runner.py`](../../eval/scripts/lib/canonical_runner.py) | symbols: `class RunItem`, `def build_schedule`, `def verify_result`, `def summarize_capacity_knee`
- **Source:** [`eval/analysis/src/wafer_analysis/results_layout.py`](../../eval/analysis/src/wafer_analysis/results_layout.py) | symbols: `class ResultsLayout`, `def resolve_raw_relative`
- **Test:** [`eval/scripts/tests/test_canonical_matrix.py`](../../eval/scripts/tests/test_canonical_matrix.py) | symbols: `def test_matrix_accepts_frozen_experiments`, `def test_final_capacity_repetitions_cannot_drop_below_30`

## Checkpoint

Choose one figure from an analysis notebook and trace it back to its batch, the physical result leaves it reads, their independent run indexes, nested observations, and evidence class. Then explain why fifty swap events in each of five runs still means N=5 rather than N=250.
