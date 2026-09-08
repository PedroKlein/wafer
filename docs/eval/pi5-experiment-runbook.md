# Run the final Raspberry Pi 5 evaluation

This how-to covers schedule inspection, pre-final validation, resumable execution, additive retrieval, contract verification, and analysis. Complete [Pi 5 host setup](pi5-host-setup.md) before using it.

The final campaign is not authorized by this document. A human-approved launch receipt is required. Until that receipt exists, `campaign_started=false`.

## Evidence classes

| Class | Purpose | Thesis evidence |
|---|---|---|
| `canonical-primary` | Execute an already frozen primary estimand after final approval | yes, after every gate passes |
| `candidate-supplementary` | Rehearse a proposed v5 experiment or additive bounded view | no; selection is pending |
| `diagnostic` | Check deployment, host admission, storage, profiling, or a changed path | no |
| `future-work` | Record evidence that is outside the current Raspberry Pi 5 campaign | no |

A directory name does not determine evidence class. Canonical-primary evidence requires the approved tag/SHA and batch ID, clean provenance, no throttling, complete matrix N, valid artifacts, and fail-closed canonical analysis. Candidate-supplementary and diagnostic runs remain separate from canonical-primary results and prior rehearsals. Candidates are not automatically admitted to N=30. A post-rehearsal selection receipt must record `include`, `defer`, or `reject` before any candidate can enter a later campaign.

## Fixed host boundary

- Raspberry Pi 5 4 GB, stock clocks, active cooling, Raspberry Pi OS Lite 64-bit.
- CPU 0: Linux support work, Mosquitto, load generation, subscription, and telemetry.
- CPUs 1-3: exactly one active SUT.
- Native eKuiper 2.1.0; no container in canonical comparisons.
- Pipeline A: `MQTT source -> threshold filter -> MQTT sink`.
- PMIC data: internal-rail proxy only, not total board or USB-C input power.

## Inspect and validate the final schedule

From the repository root on the analysis machine:

```sh
python3 eval/scripts/validate-canonical.py matrix eval/canonical-matrix.json
python3 eval/scripts/lib/canonical_runner.py \
  --dry-run \
  --batch-id final-candidate \
  --seed 1729
```

Expected matrix output:

```text
27 experiments
schedule_records=2105
measured_leaves=1893
```

The 2,105 records include 212 shared-result aliases. The 1,893 measured-or-static leaves are the processes/static measurements that produce new evidence. The deterministic schedule is written only during execution, under `eval/results/canonical-batches/rpi5-<batch-id>/schedule.json`.

The current estimate is:

| Estimate | Value | Basis |
|---|---:|---|
| Nominal active-run time | 45.52 h | Sum of matrix warmup and measurement durations for executed leaves; static E-Density-1 has zero duration |
| Operational estimate | 50.07 h | Nominal time plus 10 percent for setup, teardown, validation, and cooling |
| Storage estimate | 43.04 GiB | Conservative accounting over all 2,105 schedule records at the immutable v16 average of 17,564,330 bytes per leaf, plus 25 percent margin; shared aliases normally consume less |
| Required free space | at least 50 GiB; 60 GiB preferred | Allows attempt evidence and operational headroom |

Reserve a three-day window so the run can stop safely and resume without compressing cooling periods.

## Verify the release candidate locally

Run all gates on one clean commit before creating the evaluation tag:

```sh
cargo fmt --all -- --check
WAFER_SKIP_DOCKER_TESTS=1 cargo test --workspace
python3 -m pytest -q eval/scripts/tests
cd eval/analysis
uv sync
uv run pytest -q
cd ../..
python3 eval/scripts/check-current-docs.py
python3 eval/scripts/validate-canonical.py matrix eval/canonical-matrix.json
```

Create the release receipt only after both repository revisions are clean. Any source, config, or documentation change after the tag requires a new tag and receipt.

## Deploy and run smoke checks

Deploy the tagged release using the reviewed deployment command in the release receipt. On the Pi checkout:

```sh
./eval/scripts/preflight-pi5.sh
./eval/scripts/run-rpi5-smoke.sh
./eval/scripts/run-rpi5-validation.sh
./eval/ekuiper/seed-pipeline-a.sh
./eval/ekuiper/smoke-test.sh
```

Do not continue if preflight reports a dirty or untagged source, a non-performance governor, missing CPU isolation, an active competing SUT, insufficient disk, unavailable telemetry, or a nonzero throttling state.

## Run the targeted pre-final pilot

The targeted pilot is a new diagnostic batch declared before launch. It covers:

- E-Val-1;
- all four E-Perf-7 modes;
- all E-Perf-10 systems and rates;
- all three E-Swap-3 strategies;
- true-burst E-Swap-4.

Its schedule, repetition count, batch ID, tag/SHA, and matrix hash belong in the targeted-pilot receipt. Targeted results use `thesis_evidence=false` and are never pooled with the final batch. Stop, preserve the failed attempt, fix and retag if metering truth, capacity counters, event alignment, E-Swap-4 phase or drain boundaries, provenance, or throttling fails. E-Swap-4 must retain 1,200 source-origin primary buckets over `[0,120s)` and 100 separate drain buckets over `[120s,130s)`; full-run counters reconcile both regions. Reconciled drain arrivals are reported, while an after-drain receive, source completion at or after 130 seconds, or incomplete population fails closed.

The targeted pilot must also demonstrate resume behavior: stop after a declared leaf, rerun the same command, and confirm passed attempts are skipped rather than duplicated.

## Human approval gate

Before the full run, review the release receipt, targeted-pilot review, final schedule, runtime/storage estimates, stop conditions, and E-Perf-5 x86 limitation. The approval receipt must include at least:

- `decision="APPROVE"` and approval timestamp;
- fresh batch ID;
- WAFER tag and SHA;
- tcc-doc SHA;
- canonical matrix and schedule SHA-256;
- binary and plugin receipts;
- targeted-pilot ID;
- expected runtime and storage;
- `campaign_started=false`.

Canonical analysis consumes a subset of this receipt through `WAFER_FULL_RUN_APPROVAL`. Approval authorizes a later launch; it does not start one.

## Launch and resume the final batch

Use the exact command copied into the approved handoff. Its shape is:

```sh
./eval/scripts/run-rpi5-canonical.sh \
  --execute \
  --batch-id <approved-fresh-batch-id> \
  --seed 1729
```

Run only after explicit approval. The runner executes sequentially, writes `progress.jsonl`, creates a new attempt directory after failure, and skips a run index once a passed attempt exists. Reusing the same command and batch ID resumes the batch.

To dry-run one experiment without execution:

```sh
./eval/scripts/run-rpi5-canonical.sh \
  --dry-run \
  --batch-id preview-only \
  --seed 1729 \
  --experiments e-swap-3
```

## Monitor and stop safely

Monitor the batch ledger and telemetry without changing result files:

```sh
tail -f eval/results/canonical-batches/rpi5-<batch-id>/progress.jsonl
find eval/results -path "*rpi5-<batch-id>*" -name canonical-status.json -print
```

Stop the runner normally with SIGINT. Do not delete partial attempts. Stop admission immediately for:

- thermal throttling or missing telemetry;
- provenance, config, binary, or plugin drift;
- disk below the release-receipt floor;
- repeated systemic harness failure;
- counter or schema mismatch;
- missed E-Swap event alignment;
- unexpected concurrent SUT activity.

A threshold miss by a valid SUT run is data, not a reason to tune the threshold or system during the batch.

## Move the single evidence volume between hosts

V8 raw evidence has one physical copy on the exFAT volume labeled `WAF_RESULTS`. Mount it at `/mnt/wafer-results` on Pi or Jetson and `/Volumes/WAF_RESULTS` on macOS. Manifests contain paths relative to the volume root, never host-specific absolute paths.

On the Pi, create the SHA-256 manifest under `manifests/` from volume-root-relative raw paths. Verify it on the Pi before handoff. Then stop every writer, run `sync`, and unmount the volume cleanly. Do not unplug a mounted or busy volume.

After physically moving the drive, mount the same filesystem on macOS and confirm its UUID and label. Verify the same manifest in place before analysis reads any file. Repeat checksum verification after every host transition, including a return to Pi or a later Jetson check. A failed checksum, unexpected file, missing file, stale mount, or unclean unmount blocks use of the evidence.

Raw evidence is append-only and retains failed and interrupted attempts. Analysis opens `raw/` read-only and writes only to `derived/` and `reports/`. Do not use `rsync`, Finder, hardlinks, symlinks, or another disk to create a second raw copy. Receipts and content-free manifests may be committed to the repository; raw artifacts remain on `WAF_RESULTS`.

## Verify contracts

Verify every retrieved experiment batch:

```sh
python3 eval/scripts/verify-result-contract.py --canonical \
  eval/results/<experiment>/rpi5-<batch-id>
```

Then re-run matrix validation and compare the accepted run population with `schedule.json`. All required conditions must have the declared N; shared aliases must point to their declared source leaf.

## Analyze an approved complete batch

```sh
cd eval/analysis
uv sync
export WAFER_FULL_RUN_APPROVAL=../../.plans/rpi5-final-experiment-readiness/full-run-approval.json
export WAFER_EVAL_BATCH_ID=<batch-id>
export WAFER_ANALYSIS_OUTPUT_DIR=figures/final-<batch-id>
uv run pytest -q
uv run jupyter nbconvert --execute --to notebook --output-dir /tmp \
  notebooks/09-saturation.ipynb
```

Canonical analysis rejects an unapproved, incomplete, wrong-host, dirty, mixed-SHA, throttled, failed, or malformed batch. Diagnostic paths may render `PENDING`, but cannot become thesis evidence.

## Experiment boundaries to retain

- E-Perf-1 is the matched 1,000 msg/s operating point, not capacity.
- E-Perf-10 is the common-grid gateway envelope with MQTT support censoring. Delivery-good remains pooled loss at or below 1 percent, mean achieved/offered ratio at least 0.99, and zero duplicates.
- E-Perf-7 disables mechanisms by TOML omission.
- E-Perf-9 is Linux filesystem page-cache evidence with disk compiled-component cache disabled.
- E-Perf-5 remains `PENDING` until the matched x86 Linux block exists; x86 execution and any cross-architecture conclusion are `future-work`.
- E-Swap-3 uses actual-t0-aligned 100 ms output buckets.
- E-Swap-4 has one source-driven burst and one stateless swap per independent run; primary `[0,120s)` and drain `[120s,130s)` sink evidence remain separate and strictly reconciled, with zero after-drain arrivals and no right censoring.
- E-Perf-2 remains an alias view of E-Perf-1, E-Perf-8 of E-Perf-6, and E-Swap-2/E-Swap-6 of E-Swap-1. Aliases never add independent samples.
- PMIC remains an internal-rail proxy, not total input power. External USB-C input-power capture is `future-work`.
- Admission with the retained 5 V / 4.2 A supply is empirical. It receives no threshold waiver for throttling, temperature, reboot, or I/O failure.
- Diagnostic scout, v11-v17, targeted-pilot, laptop, synthetic, and candidate-supplementary data are not pooled with canonical-primary results.
