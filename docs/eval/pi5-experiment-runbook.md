# Run the final Raspberry Pi 5 evaluation

This how-to covers schedule inspection, local verification, a diagnostic batch, resumable execution, additive retrieval, contract verification, batch approval, and analysis. Complete [Pi 5 host setup](pi5-host-setup.md) before using it.

Launching the final batch is a human decision, taken after the diagnostic batch passes review. A finished batch becomes thesis evidence only after `mise run approve-batch` records it in `eval/final-batches.json` and that file is committed.

## Evidence classes

| Class | Purpose | Thesis evidence |
|---|---|---|
| `canonical-primary` | Execute an already frozen primary estimand in the final batch | yes, once the batch passes every gate and is approved |
| `candidate-supplementary` | Rehearse a proposed v5 experiment or additive bounded view | no; selection is pending |
| `diagnostic` | Check deployment, host admission, profiling, or a changed path | no |
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
schedule_records=2351
measured_leaves=2121
```

The 2,351 records include shared-result aliases with `independent_n_contribution=0`. The 2,121 measured-or-static leaves are the processes/static measurements that produce new evidence. These counts are the common schedule that every host shares. Each host's final batch adds its E-Perf-10 bracket rates (see [Run the host's capacity scout](#run-the-hosts-capacity-scout)): 120 leaves per rate, at most six rates, so at most 720 leaves. The `BRACKETS` line of a dry run with `--scout-batch-id`, and the batch's `batch.json`, give the host's own figure. Without `--scout-batch-id`, as above, the dry run prints the common schedule and a `NOTE` that `--execute` refuses to start that final batch. The deterministic schedule is written only during execution, under `eval/results/canonical-batches/rpi5-<batch-id>/schedule.json`.

The current estimate is:

| Estimate | Value | Basis |
|---|---:|---|
| Nominal active-run time | 51.13 h | Sum of matrix warmup and measurement durations for executed leaves; static E-Density-1 has zero duration |
| Operational estimate | 56.25 h | Nominal time plus 10 percent for setup, teardown, validation, and cooling |
| Bracket rates, per host | at most 18.00 h nominal, 19.80 h operational | 3.00 h nominal per rate (120 runs of 90 s) on top of the common schedule above, at most six rates |
| Storage estimate | 43,433,437,093 bytes | Primary N=30 estimate over the earlier 2,165 schedule records, including the three E-Backpressure policies; it predates the 120 E-Perf-4 native runs and the ten-run E-Swap-1 and E-Swap-5 sessions, so regenerate before execution |
| Required free space | at least 50 GiB; 60 GiB preferred | Allows attempt evidence and operational headroom |

Reserve a three-day window, plus 3.3 hours for each bracket rate, so the run can stop safely and resume without compressing cooling periods.

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
python3 eval/scripts/validate-canonical.py matrix eval/canonical-matrix.json
```

The first `cargo test` downloads the pinned ONNX Runtime archive (default `ort-download` feature). Without network access, set `ORT_LIB_LOCATION` to a local ONNX Runtime build first; see [ONNX Runtime](../operations/dependencies.md#onnx-runtime).

E-Density-1 compares the plugin components with a measured container floor, and that floor must be committed before these gates run. On a machine with Docker that can run images for the platform (natively or through QEMU emulation), measure it for each host architecture and commit the files it writes:

```sh
python3 eval/scripts/measure-container-floor.py --platform linux/arm64   # Pi 5 and Jetson
python3 eval/scripts/measure-container-floor.py --platform linux/amd64   # x86
```

The tool builds a `FROM scratch` image that holds only a statically linked Rust pass-through worker (`eval/container-floor/`), checks that the image passes its input through, and writes `eval/container-floor/linux-arm64.json` or `linux-amd64.json` with the image size and the pinned build inputs. The E-Density-1 collector copies the file for its host into the result and fails without it, and host preflight warns when it is missing, so the other experiments can still run. `python3 -m pytest -q eval/scripts/tests` fails when a committed floor no longer matches the worker sources, the Dockerfile or `rust-toolchain.toml`; measure again after changing any of them.

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

Use `--host jetson` or `--host x86` on the replication hosts. The smoke test confirms that the rule passes only the boundary record, with its schema and `ts`/`seq` unchanged. The E-Swap-3 batch then runs five repetitions of each arm under the 1,000 msg/s Pipeline A load: five `ekuiper-rule-update` runs that update `pipeline_a` once with `PUT /rules/pipeline_a` and `"triggered": false`, then start it with `POST /rules/pipeline_a/start`, five `ekuiper-make-before-break` runs that replace it with `pipeline_a_v2`, and the two WAFER arms alongside: 20 runs, about an hour. Each eKuiper run first refuses a package other than 2.1.5, a rule list other than `pipeline_a` alone, and a `pipeline_a` other than the rule the seed script creates. A rule update fails its run if the PUT or the start does not return 200 or the updated rule does not count output within 10 seconds, and a make-before-break replacement fails its run if a REST call fails or `pipeline_a_v2` does not count output within 10 seconds. The command exits non-zero when a run failed for the harness, and the batch's `failures.json` lists those runs. A run in which the `kuiper` unit restarted or the rule the run ends with stopped or reported an error is a system outcome instead: the command still exits 0, prints an `OUTCOME` line with `runtime-exit` or `rule-error`, and the run's `canonical-status.json` records it. Investigate any failure or outcome before running that host's diagnostic or final batch. The re-check batch is diagnostic: its `batch.json` records `repetitions=5` and `thesis_evidence=false`, `approve-batch` refuses it, and it is never pooled with the final batch.

## Smoke-run the E-Swap-3 arms before each batch

Before the diagnostic batch, and again before the final batch, run every E-Swap-3 arm once from the deployed commit:

```sh
mise run smoke-swap3 -- --batch-id swap3-smoke-<date>
```

The task runs `./eval/scripts/run-rpi5-canonical.sh --execute --experiments e-swap-3 --repetitions 1` with the arguments given; add `--host jetson` or `--host x86` on the replication hosts. That is four runs, one for each of `wafer-hotswap`, `wafer-restart`, `ekuiper-rule-update` and `ekuiper-make-before-break`, and about ten minutes of nominal run time. It exercises what only one arm uses: the hot swap to `threshold-filter-v2` and its adopted response in `swap_requests.json`, the WAFER restart, the eKuiper rule update (PUT with `triggered` false, then start) and its REST calls in `rule-update.json`, and the make-before-break REST calls in `rule-replacement.json`. The batch is diagnostic: its `batch.json` records `repetitions=1` and `thesis_evidence=false`, `approve-batch` refuses it, and its results are never pooled with another batch. When a run fails for the harness or prints an `OUTCOME` line, investigate it before starting the batch. A `wafer-hotswap` run fails for the harness before warm-up when `threshold-filter-v2` is not built on the host; build the plugins with `mise run //plugins:build-plugins` and run the smoke test again.

## Run the host's capacity scout

Each host's final batch adds a few E-Perf-10 rates beside the common grid, taken from that host's own capacity scout. Run the scout on the host, from the deployed commit, before its diagnostic batch:

```sh
./eval/scripts/run-rpi5-canonical.sh \
  --execute \
  --host rpi5 \
  --capacity-scout \
  --batch-id scout-<date>
```

Each run of the command carries out the next probe decision and exits. Run it again until it prints `"action": "stop"` and writes `manifests/capacity-scout/rpi5-scout-<date>/scout-complete.json` in the host's results root. Then preview what the final batch will add:

```sh
./eval/scripts/run-rpi5-canonical.sh \
  --dry-run \
  --host rpi5 \
  --batch-id preview-only \
  --scout-batch-id scout-<date>
```

The `BRACKETS` line lists the bracket rates, the scout summary and its SHA-256, and the leaves and nominal hours the rates add. [Per-host bracket rates](../../eval/RESULT-CONTRACT.md#per-host-bracket-rates) defines the rule. Use `--host jetson` or `--host x86` on the replication hosts; each host uses its own scout. The scout is diagnostic and is never pooled with the final batch.

## Run the diagnostic batch

Before the final batch, run the first three repetitions of every experiment from the deployed commit:

```sh
./eval/scripts/run-rpi5-canonical.sh \
  --execute \
  --batch-id diag-<date> \
  --seed 1729 \
  --scout-batch-id scout-<date> \
  --repetitions 3
```

That is 217 runs and about five and a half hours of nominal run time, plus 12 runs and 18 minutes for each bracket rate. Without `--scout-batch-id` the diagnostic batch runs the common E-Perf-10 grid only. The batch is diagnostic. Its `batch.json` records `repetitions=3` and `thesis_evidence=false`, every leaf carries `thesis_evidence=false`, and `approve-batch` refuses it. Its results are never pooled with the final batch. Review at least:

- E-Val-1;
- all four E-Perf-7 modes;
- all E-Perf-10 systems and rates;
- all four E-Swap-3 arms, with the placebo dip next to each dip;
- true-burst E-Swap-4.

Stop and preserve the failed attempt if metering truth, capacity counters, event alignment, E-Swap-4 phase or drain boundaries, provenance, or throttling fails. Fix it in a new commit, verify and deploy that commit, and run a new diagnostic batch. E-Swap-4 must retain 1,200 source-origin primary buckets over `[0,120s)` and 100 separate drain buckets over `[120s,130s)`; full-run counters reconcile both regions. Reconciled drain arrivals are reported, while an after-drain receive, source completion at or after 130 seconds, or incomplete population fails closed.

The diagnostic batch must also demonstrate resume behavior: stop it with SIGINT after a declared leaf, rerun the same command, and confirm with `mise run campaign-status -- --batch-id diag-<date>` that admitted attempts are skipped rather than duplicated.

## Launch and resume the final batch

Launch only after the diagnostic batch passes review and a new E-Swap-3 smoke run from the same deployed commit is clean:

```sh
./eval/scripts/run-rpi5-canonical.sh \
  --execute \
  --batch-id <fresh-batch-id> \
  --seed 1729 \
  --scout-batch-id scout-<date>
```

`mise run run-campaign -- --batch-id <id>` runs the same command, and `mise run campaign-status -- --batch-id <id>` lists its done and pending runs. On the Jetson and x86 replication hosts add `--host jetson` or `--host x86`.

On the first start the runner writes `batch.json` into the batch ledger. It records the batch ID, host, source SHA and dirty flag, the SHA-256 of `eval/canonical-matrix.json`, the seed, the experiments, the repetition override, whether the batch can become thesis evidence, the start time, and `capacity_brackets`: the scout batch, the SHA-256 of its `scout-complete.json`, the E-Perf-10 bracket rates derived from it, and the leaves and nominal hours they add. The runner also copies that `scout-complete.json` into the ledger. A final batch refuses to start without a usable scout summary for its host, and a resume derives the rates from the ledger copy and refuses the batch when the copy or the rates it gives have changed. Running the scout command again does not affect a batch that has started. The runner executes sequentially and writes `progress.jsonl`. Each attempt ends as a clean pass, a system outcome, or an infrastructure failure, and its `canonical-status.json` records the class and reasons; [Attempts and retries](../../eval/RESULT-CONTRACT.md#attempts-and-retries) defines them. A clean pass or a system outcome is admitted and the run index is skipped from then on. A system outcome, such as a runtime exit or a containment escape, is data and is never retried. An infrastructure failure is retried once, immediately and in a new attempt directory; when the retry also fails, the run is missing and is not run again. E-Val-1 is never retried: any failed or interrupted repetition fails the gate, and a new gate needs a new batch. Reusing the same command and batch ID resumes the batch; an interrupted attempt counts as one infrastructure attempt. `--status` lists done, pending and missing runs. A resume from another commit or matrix is refused. A final batch also refuses to start or resume from a dirty source tree, and a batch that `approve-batch` has sealed cannot be resumed.

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

Stop the runner normally with SIGINT. Do not delete partial attempts. The runner records a system outcome and moves on; it does not stop for one. Stop admission immediately for:

- thermal throttling or missing telemetry;
- provenance, config, binary, or plugin drift;
- free disk space below the required free space in the estimate above;
- repeated systemic harness failure, including missing runs whose retry also failed;
- counter or schema mismatch;
- missed E-Swap event alignment;
- unexpected concurrent SUT activity.

A threshold miss, a crash, or a containment escape by the system under test is data, not a reason to rerun the unit or to tune the threshold or system during the batch.

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
- the bracket rates in `batch.json` still follow from the copy of the host's scout summary in the ledger;
- `schedule.json` is the full schedule for the batch seed and bracket rates, and every run has an admitted attempt (a clean pass or a system outcome) within its retry cap, or an alias receipt;
- `e-val-1-gate.json` reports a pass;
- every admitted leaf has the batch SHA, a clean tree, `throttled=0x0`, and no `thesis_evidence=false`.

It then writes `raw.sha256` into the ledger. That file holds the SHA-256 of every file of the batch under `raw/`, its alias receipts, and its ledger, including the copy of the scout summary its bracket rates came from, with paths relative to the volume root. Check it at any time:

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
- E-Perf-10 is the gateway envelope over the common grid plus the host's bracket rates, with MQTT support censoring. Delivery-good remains pooled loss at or below 1 percent, mean achieved/offered ratio at least 0.99, and zero duplicates. Each delivery ceiling is bracketed by tested rates, and a WAFER/eKuiper ratio interval that straddles 0.70 is `CENSORED`.
- E-Perf-7 disables mechanisms by TOML omission.
- E-Perf-9 is Linux filesystem page-cache evidence with disk compiled-component cache disabled.
- E-Perf-5 remains `PENDING` until the matched x86 Linux block exists; x86 execution and any cross-architecture conclusion are `future-work`.
- E-Swap-3 uses actual-t0-aligned 100 ms output buckets.
- E-Swap-1 and E-Swap-5 have ten independent process runs with 50 nested swap or rollback events each. Analysis summarises each run first and reports the first-use (compiling) event of each run apart from the cached events.
- E-Swap-4 has one source-driven burst and one stateless swap per independent run; primary `[0,120s)` and drain `[120s,130s)` sink evidence remain separate and strictly reconciled, with zero after-drain arrivals and no right censoring.
- E-Perf-2 remains an alias view of E-Perf-1, E-Perf-8 of E-Perf-6, and E-Swap-2/E-Swap-6 of E-Swap-1. Aliases never add independent samples.
- PMIC remains an internal-rail proxy, not total input power. External USB-C input-power capture is `future-work`.
- The retained 5 V / 4.2 A supply gets no threshold waiver: every final run must record `throttled=0x0`, and `approve-batch` refuses a batch with any other value.
- Diagnostic scout, v11-v17, diagnostic-batch, laptop, synthetic, and candidate-supplementary data are not pooled with canonical-primary results.
- The capacity scout's `wafer-max-inflight-1` arm runs WAFER with one MQTT publish in flight. It only tests whether eKuiper's one-at-a-time QoS 1 sink explains a capacity gap; it never feeds the E-Perf-10 grid or decision.
