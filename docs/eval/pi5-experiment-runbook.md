# Run the final Raspberry Pi 5 evaluation

This how-to covers schedule inspection, local verification, a diagnostic batch, resumable execution, additive retrieval, contract verification, batch approval, and analysis. Complete [Pi 5 host setup](pi5-host-setup.md) before using it.

Launching the final batch is a human decision, taken after the diagnostic batch passes review. A finished batch becomes thesis evidence only after `mise run approve-batch` records it in `eval/final-batches.json` and that file is committed.

## Evidence classes

| Class | Purpose | Thesis evidence |
|---|---|---|
| `canonical-primary` | Execute an already frozen primary estimand in the final batch | yes, once the batch passes every gate and is approved |
| `candidate-supplementary` | Rehearse a proposed v5 experiment or additive bounded view | no; selection is pending |
| `diagnostic` | Check deployment, host admission, storage, profiling, or a changed path | no |
| `future-work` | Record evidence that is outside the current Raspberry Pi 5 campaign | no |

A directory name does not determine evidence class. Canonical-primary evidence requires one clean source commit for the whole batch, an entry for the batch in `eval/final-batches.json`, no throttling, complete matrix N, valid artifacts, and fail-closed canonical analysis. Candidate-supplementary and diagnostic runs remain separate from canonical-primary results and prior rehearsals. Candidates are not automatically admitted to N=30; a candidate enters the final campaign only by moving it into the matrix's final `experiments` list before a final batch starts.

## Fixed host boundary

- Raspberry Pi 5 4 GB, stock clocks, active cooling, Raspberry Pi OS Lite 64-bit.
- CPU 0: Linux support work, Mosquitto, load generation, subscription, and telemetry.
- CPUs 1-3: exactly one active SUT.
- Native eKuiper 2.1.5; no container in canonical comparisons.
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
schedule_records=2165
measured_leaves=1953
```

The 2,165 records include shared-result aliases with `independent_n_contribution=0`. The 1,953 measured-or-static leaves are the processes/static measurements that produce new evidence. The deterministic schedule is written only during execution, under `eval/results/canonical-batches/rpi5-<batch-id>/schedule.json`.

The current estimate is:

| Estimate | Value | Basis |
|---|---:|---|
| Nominal active-run time | 45.52 h | Sum of matrix warmup and measurement durations for executed leaves; static E-Density-1 has zero duration |
| Operational estimate | 50.07 h | Nominal time plus 10 percent for setup, teardown, validation, and cooling |
| Storage estimate | 43,433,437,093 bytes | Current primary N=30 estimate over all 2,165 schedule records, including the three E-Backpressure policies; regenerate before execution |
| Required free space | at least 50 GiB; 60 GiB preferred | Allows attempt evidence and operational headroom |

Reserve a three-day window so the run can stop safely and resume without compressing cooling periods.

## Verify one clean commit locally

Run all gates on the commit you will deploy. The tree must be clean. No tag or release receipt is needed: the runner records the commit SHA in the batch ledger, and every leaf records it in `metadata.json`.

```sh
cargo fmt --all -- --check
mise run test
python3 -m pytest -q eval/scripts/tests
cd eval/analysis
uv sync
uv run pytest -q
cd ../..
python3 eval/scripts/check-current-docs.py
python3 eval/scripts/validate-canonical.py matrix eval/canonical-matrix.json
```

The first `cargo test` downloads the pinned ONNX Runtime archive (default `ort-download` feature). Without network access, set `ORT_LIB_LOCATION` to a local ONNX Runtime build first; see [ONNX Runtime](../operations/dependencies.md#onnx-runtime).

E-Density-1 compares the plugin components with a measured container floor, and that floor must be committed before these gates run. On a machine with Docker that can run images for the platform (natively or through QEMU emulation), measure it for each host architecture and commit the files it writes:

```sh
python3 eval/scripts/measure-container-floor.py --platform linux/arm64   # Pi 5 and Jetson
python3 eval/scripts/measure-container-floor.py --platform linux/amd64   # x86
```

The tool builds a `FROM scratch` image that holds only a statically linked Rust pass-through worker (`eval/container-floor/`), checks that the image passes its input through, and writes `eval/container-floor/linux-arm64.json` or `linux-amd64.json` with the image size and the pinned build inputs. The E-Density-1 collector copies the file for its host into the result and fails without it, and host preflight reports a missing file as a failure. `python3 -m pytest -q eval/scripts/tests` fails when a committed floor no longer matches the worker sources, the Dockerfile or `rust-toolchain.toml`; measure again after changing any of them.

Any source, config, or documentation change after this point is a new commit. Run the gates again and start a new batch. The runner refuses to resume a batch from another commit or another `eval/canonical-matrix.json`.

## Deploy and run smoke checks

Deploy the verified commit with `./eval/scripts/deploy-pi5.sh --host USER@wafer-pi5` (see [Pi 5 host setup](pi5-host-setup.md)). The bundle carries `SOURCE_STATE.json` with the commit SHA and dirty flag. On the Pi, the runner and the leaf metadata take the source SHA from that file. Then run on the Pi:

```sh
./eval/scripts/preflight-pi5.sh
./eval/scripts/run-rpi5-smoke.sh
./eval/scripts/run-rpi5-validation.sh
./eval/ekuiper/seed-pipeline-a.sh
./eval/ekuiper/smoke-test.sh
```

Do not continue if preflight reports a dirty source, a non-performance governor, a failed CPU affinity or load-balancing check, an active competing SUT, insufficient disk, unavailable telemetry, or a nonzero throttling state.

Before the final batch, record the idle-power baseline once on the same host state (broker up, no pipeline):

```sh
./eval/scripts/run-rpi5-idle-baseline.sh   # or: mise run idle-baseline-pi5
```

It stops `kuiper.service` as WAFER runs do (and restarts it on exit), refuses to start while a `wafer` or `wafer-loadgen` process exists, takes ten 60 s samples with the two host sidecars only, and writes `eval/results/idle-baseline/rpi5-<UTC>/idle-baseline.json` with the median PMIC internal-rail proxy watts, the SUT-core busy fraction that proves the host was idle, and the throttle state. The baseline is diagnostic, never thesis evidence. Pass it to the proxy energy report with `canonical_power.py --idle-baseline <dir>/idle-baseline.json`, which refuses a baseline marked not usable or throttled.

Also measure what the host telemetry sidecars cost on the Pi:

```sh
./eval/scripts/run-rpi5-instrument-ab.sh   # or: mise run instrument-ab-pi5
```

It runs six pairs of E-Perf-1 WAFER runs (the canonical 30 s warmup, 60 s measurement, 60,000 messages, same cpusets), one with the Pi and `/proc` sidecars on and one with them off (`WAFER_HOST_SIDECARS=off`), alternating which goes first and pausing 60 s between runs, with eKuiper stopped in both arms. `analyze-instrument-ab.py` writes `instrument-ab.json` with the paired on-minus-off differences of p50, p95, p99 and achieved rate and the `/proc` sampler's CPU share, and exits non-zero unless the median p95 and achieved-rate differences are within 5%, every run is lossless and the sampler used at most 1% of one core. The pairs are diagnostic and never pooled with the final batch. About 25 minutes of Pi time.

## Re-check the eKuiper comparator before each batch

Run this on every host (Raspberry Pi 5, Jetson and x86) after deploying the commit it will measure and before its diagnostic batch, and again whenever its eKuiper package, OS, or deployed commit changes. A host that still runs an older eKuiper package gets 2.1.5 from `./eval/ekuiper/install-native.sh`, which upgrades it in place and restarts the service. Then run on that host:

```sh
./eval/ekuiper/seed-pipeline-a.sh
./eval/ekuiper/smoke-test.sh
./eval/scripts/run-rpi5-canonical.sh \
  --execute \
  --host rpi5 \
  --batch-id ekuiper-recheck-<date> \
  --experiments e-swap-3 \
  --repetitions 5
```

Use `--host jetson` or `--host x86` on the replication hosts. The smoke test confirms that the rule passes only the boundary record, with its schema and `ts`/`seq` unchanged. The E-Swap-3 batch then stops and starts `pipeline_a` once in each of five `ekuiper-restart` runs under the 1,000 msg/s Pipeline A load, and runs the two WAFER strategies alongside: 15 runs, about 45 minutes. Each eKuiper run first refuses a package other than 2.1.5, and a restart fails its run if either REST call fails or the rule does not report `running` within 10 seconds. The command exits 0 only when every run passed; otherwise the batch's `failures.json` lists the failed runs. Investigate any failure before running that host's diagnostic or final batch. The re-check batch is diagnostic: its `batch.json` records `repetitions=5` and `thesis_evidence=false`, `approve-batch` refuses it, and it is never pooled with the final batch.

## Run the diagnostic batch

Before the final batch, run the first three repetitions of every experiment from the deployed commit:

```sh
./eval/scripts/run-rpi5-canonical.sh \
  --execute \
  --batch-id diag-<date> \
  --seed 1729 \
  --repetitions 3
```

That is 198 runs and about five hours of nominal run time. The batch is diagnostic. Its `batch.json` records `repetitions=3` and `thesis_evidence=false`, every leaf carries `thesis_evidence=false`, and `approve-batch` refuses it. Its results are never pooled with the final batch. Review at least:

- E-Val-1;
- all four E-Perf-7 modes;
- all E-Perf-10 systems and rates;
- all three E-Swap-3 strategies;
- true-burst E-Swap-4.

Stop and preserve the failed attempt if metering truth, capacity counters, event alignment, E-Swap-4 phase or drain boundaries, provenance, or throttling fails. Fix it in a new commit, verify and deploy that commit, and run a new diagnostic batch. E-Swap-4 must retain 1,200 source-origin primary buckets over `[0,120s)` and 100 separate drain buckets over `[120s,130s)`; full-run counters reconcile both regions. Reconciled drain arrivals are reported, while an after-drain receive, source completion at or after 130 seconds, or incomplete population fails closed.

The diagnostic batch must also demonstrate resume behavior: stop it with SIGINT after a declared leaf, rerun the same command, and confirm with `mise run campaign-status -- --batch-id diag-<date>` that passed attempts are skipped rather than duplicated.

## Launch and resume the final batch

Launch only after the diagnostic batch passes review, from the same deployed commit:

```sh
./eval/scripts/run-rpi5-canonical.sh \
  --execute \
  --batch-id <fresh-batch-id> \
  --seed 1729
```

`mise run run-campaign -- --batch-id <id>` runs the same command, and `mise run campaign-status -- --batch-id <id>` lists its done and pending runs. On the Jetson and x86 replication hosts add `--host jetson` or `--host x86`.

On the first start the runner writes `batch.json` into the batch ledger. It records the batch ID, host, source SHA and dirty flag, the SHA-256 of `eval/canonical-matrix.json`, the seed, the experiments, the repetition override, whether the batch can become thesis evidence, and the start time. The runner executes sequentially, writes `progress.jsonl`, creates a new attempt directory after failure, and skips a run index once a passed attempt exists. Reusing the same command and batch ID resumes the batch. A resume from another commit or matrix is refused. A final batch also refuses to start or resume from a dirty source tree, and a batch that `approve-batch` has sealed cannot be resumed.

To dry-run one experiment without execution:

```sh
./eval/scripts/run-rpi5-canonical.sh \
  --dry-run \
  --batch-id preview-only \
  --seed 1729 \
  --experiments e-swap-3
```

Candidate experiments run as their own batch with `--experiments candidates`. The runner refuses to mix them with final experiments and writes their ledger under `manifests/candidate-batches/`.

## Monitor and stop safely

Monitor the batch ledger and telemetry without changing result files:

```sh
tail -f eval/results/canonical-batches/rpi5-<batch-id>/progress.jsonl
find eval/results -path "*rpi5-<batch-id>*" -name canonical-status.json -print
```

Stop the runner normally with SIGINT. Do not delete partial attempts. Stop admission immediately for:

- thermal throttling or missing telemetry;
- provenance, config, binary, or plugin drift;
- free disk space below the required free space in the estimate above;
- repeated systemic harness failure;
- counter or schema mismatch;
- missed E-Swap event alignment;
- unexpected concurrent SUT activity.

A threshold miss by a valid SUT run is data, not a reason to tune the threshold or system during the batch.

## Move the single evidence volume between hosts

V9 raw evidence has one physical copy on the exFAT volume labeled `WAF_RESULTS`. Mount it at `/mnt/wafer-results` on Pi or Jetson and `/Volumes/WAF_RESULTS` on macOS. Manifests contain paths relative to the volume root, never host-specific absolute paths.

On the Pi, create the SHA-256 manifest under `manifests/` from volume-root-relative raw paths and verify it there. Then stop every writer, run `sync`, and unmount the volume cleanly. Do not unplug a mounted or busy volume, and do not edit anything under `raw/`.

After physically moving the drive, mount the same filesystem on macOS and confirm its UUID and label. Verify the same manifest in place before analysis reads any file. Repeat checksum verification after every host transition, including a return to Pi or a later Jetson check. A failed checksum, unexpected file, missing file, stale mount, or unclean unmount blocks use of the evidence.

Raw evidence is append-only and retains failed and interrupted attempts. Analysis opens `raw/` read-only and writes only to `derived/` and `reports/`. Do not use `rsync`, Finder, hardlinks, symlinks, or another disk to create a second raw copy. Receipts and content-free manifests may be committed to the repository; raw artifacts remain on `WAF_RESULTS`.

## Verify contracts

Verify every retrieved experiment batch:

```sh
python3 eval/scripts/verify-result-contract.py --canonical \
  eval/results/<experiment>/rpi5-<batch-id>
```

Then re-run matrix validation and compare the accepted run population with `schedule.json`. All required conditions must have the declared N; shared aliases must point to their declared source leaf.

## Approve the finished batch

On the analysis machine, with the volume mounted and the repository at a commit with the same `eval/canonical-matrix.json`:

```sh
WAFER_RESULTS_ROOT=/Volumes/WAF_RESULTS \
  mise run approve-batch -- --batch-id <batch-id> --host rpi5
```

It refuses the batch unless all of these hold:

- the ledger is under `manifests/canonical-batches/`, and its `batch.json` marks thesis evidence with no repetition override;
- the batch ran from one clean commit with the current matrix;
- `schedule.json` is the full schedule for the batch seed, and every run has a passed attempt or an alias receipt;
- `e-val-1-gate.json` reports a pass;
- every passed leaf has the batch SHA, a clean tree, `throttled=0x0`, and no `thesis_evidence=false`.

It then writes `raw.sha256` into the ledger. That file holds the SHA-256 of every file of the batch under `raw/`, its alias receipts, and its ledger, with paths relative to the volume root. Check it at any time:

```sh
cd /Volumes/WAF_RESULTS
shasum -a 256 -c manifests/canonical-batches/rpi5-<batch-id>/raw.sha256   # sha256sum -c on Linux
```

It also records the batch in `eval/final-batches.json`: the batch ID, the WAFER SHA, the matrix SHA-256, the SHA-256 of `raw.sha256`, the approval time, and `other_final_batches`. That last field lists every other thesis-eligible batch of the same host in the results root, so a choice among several batches stays visible. Each host has one entry. `approve-batch` refuses to replace an entry that names another batch, and approving the same batch again changes nothing. Commit `eval/final-batches.json`.

## Analyze an approved complete batch

```sh
cd eval/analysis
uv sync
export WAFER_EVAL_BATCH_ID=<batch-id>
export WAFER_ANALYSIS_OUTPUT_DIR=figures/final-<batch-id>
uv run pytest -q
uv run jupyter nbconvert --execute --to notebook --output-dir /tmp \
  notebooks/09-saturation.ipynb
```

Canonical analysis reads the host entry in `eval/final-batches.json`. It rejects a batch that is not the approved one, whose `batch.json` or `raw.sha256` differs from the entry, or that is incomplete, wrong-host, dirty, mixed-SHA, throttled, failed, or malformed. Diagnostic paths may render `PENDING`, but cannot become thesis evidence.

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
- Diagnostic scout, v11-v17, diagnostic-batch, laptop, synthetic, and candidate-supplementary data are not pooled with canonical-primary results.
