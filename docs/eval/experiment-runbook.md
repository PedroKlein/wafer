# Run and approve an evaluation batch

The thesis results come from one approved N=30 batch per host (Raspberry Pi 5,
Jetson Orin Nano and x86), each recorded in `eval/final-batches.json`. This
how-to re-runs the evaluation on a host, approves the finished batch, and
regenerates the analysis. Prepare the host first:
[Pi 5](pi5-host-setup.md), [Jetson](jetson-host-setup.md) or
[x86](x86-host-setup.md).

Commands default to the Pi 5 profile (`--host rpi5`). On the other hosts add
`--host jetson` or `--host x86` to every `run-rpi5-canonical.sh` and
`approve-batch` command; results then land in `jetson-<batch-id>` or
`x86-<batch-id>` directories. The Pi bundle carries no `mise.toml`, so commands
that run on a host call the scripts directly.

## What becomes evidence

A batch is thesis evidence only when it runs the full schedule (no
`--repetitions`), from one clean commit and one `eval/canonical-matrix.json`,
on a host that passes preflight with no throttling, and `mise run
approve-batch` has recorded it in a committed `eval/final-batches.json`.
Everything else on this page (re-checks, smoke runs, the capacity scout,
diagnostic batches, idle and instrumentation baselines) is diagnostic: its
`batch.json` records `thesis_evidence=false`, `approve-batch` refuses it, and
it is never pooled with a final batch.

The fixed host boundary is CPU 0 for the operating system, Mosquitto, load
generation and telemetry, and CPUs 1-3 for exactly one system under test.
eKuiper runs natively (2.1.5), never in a container. On the Pi, power is the
PMIC internal-rail proxy, not total board input power.

## Inspect the schedule

From the repository root:

```sh
python3 eval/scripts/validate-canonical.py matrix eval/canonical-matrix.json
python3 eval/scripts/lib/canonical_runner.py --dry-run --batch-id preview-only --seed 1729
```

The validator prints
`canonical matrix: PASS (27 experiments, schedule_records=2351, measured_leaves=2121)`.
The records include shared-result aliases that add no independent samples.
This is the schedule every host shares. A final batch adds the host's E-Perf-10
bracket rates (120 runs per rate, at most six rates) from its
[capacity scout](#run-the-hosts-capacity-scout); without `--scout-batch-id` the
dry run prints a `NOTE` that `--execute` would refuse the batch. The
deterministic schedule is written at execution time to
`$WAFER_RESULTS_ROOT/manifests/canonical-batches/<host>-<batch-id>/schedule.json`.

A full batch takes a little over two days of run time on one host, plus about
three hours per bracket rate. Allow at least 50 GiB of free space on the
results root.

## Verify one clean commit

Run the gates on the commit you will deploy, from a clean tree:

```sh
cargo fmt --all -- --check
mise run test
mise run test-eval
python3 eval/scripts/validate-canonical.py matrix eval/canonical-matrix.json
```

The first build downloads the pinned ONNX Runtime archive; offline, set
`ORT_LIB_LOCATION` first (see
[ONNX Runtime](../operations/dependencies.md#onnx-runtime)).

E-Density-1 compares plugin sizes with a measured container floor, committed
under `eval/container-floor/`. The harness tests fail when a committed floor
no longer matches its worker sources, Dockerfile or `rust-toolchain.toml`.
Re-measure after changing any of them, on a machine with Docker that can run
the target platform, and commit the result:

```sh
python3 eval/scripts/measure-container-floor.py --platform linux/arm64   # Pi 5 and Jetson
python3 eval/scripts/measure-container-floor.py --platform linux/amd64   # x86
```

Any later source, config or documentation change is a new commit: run the
gates again and start a new batch. The runner refuses to resume a batch from
another commit or another matrix, and refuses to start or resume a final
batch from a dirty tree.

## Deploy and run smoke checks

Deploy the verified commit as the host setup page describes (on the Pi,
`./eval/scripts/deploy-pi5.sh --host USER@wafer-pi5`). The bundle carries
`SOURCE_STATE.json` with the commit SHA and dirty flag, which the runner and
every leaf's metadata use. On the host:

```sh
./eval/scripts/preflight-pi5.sh      # preflight-jetson.sh or preflight-x86.sh elsewhere
./eval/scripts/run-rpi5-smoke.sh     # Pi only
./eval/scripts/run-rpi5-validation.sh   # Pi only
```

Do not continue if preflight reports any `FAIL` or its `deployed source state`
line shows `git_dirty=true`. Preflight does not check free disk space. On x86,
repeat the governor and frequency pinning from
[x86 host setup](x86-host-setup.md) after every boot.

Two optional Pi diagnostics characterize the host itself:

- `./eval/scripts/run-rpi5-idle-baseline.sh` records ten 60 s idle samples (broker up, no
  pipeline, `kuiper.service` stopped) in
  `eval/results/idle-baseline/rpi5-<UTC>/idle-baseline.json`. Pass that file
  to `canonical_power.py --idle-baseline` for the proxy energy report.
- `./eval/scripts/run-rpi5-instrument-ab.sh` runs six pairs of E-Perf-1 WAFER runs with the
  telemetry sidecars on and off and fails unless the median p95 and achieved
  rate differ by at most 5%, every run is lossless, and the sampler used at
  most 1% of one core. It takes about 25 minutes.

## Set the host's results root

Each host writes its batches to its own results root: a mounted filesystem,
or a Linux bind mount whose exact path appears in `/proc/self/mountinfo`. The
runner refuses an ordinary directory. Set the root in the shell that runs the
batch commands and check its free space:

```sh
export WAFER_RESULTS_ROOT=<this host's mounted results root>
df -h "$WAFER_RESULTS_ROOT"
```

On the Pi the root is the `WAF_RESULTS` volume at `/mnt/wafer-results` (see
[Pi 5 host setup](pi5-host-setup.md#9-mount-the-results-root)). The runner
writes each leaf under `raw/<experiment>/<host>-<batch-id>/` and each ledger
under `manifests/`; the
[result contract](../../eval/RESULT-CONTRACT.md#path-convention) defines the
layout. Without `WAFER_RESULTS_ROOT` or `--results-root` the runner falls back
to the repository-local `eval/results/`, which is for local tests only. Raw
evidence is append-only and keeps failed and interrupted attempts; never edit
anything under `raw/`.

## Re-check the eKuiper comparator before each batch

Run this on each host after deploying the commit it will measure, and again
whenever its eKuiper package, OS or deployed commit changes.
`./eval/ekuiper/install-native.sh` installs or upgrades eKuiper 2.1.5. Then:

```sh
./eval/ekuiper/seed-pipeline-a.sh
./eval/ekuiper/smoke-test.sh
./eval/scripts/run-rpi5-canonical.sh --execute --host rpi5 \
  --batch-id ekuiper-recheck-<date> --experiments e-swap-3 --repetitions 5
```

The smoke test confirms that the rule passes only the boundary record with its
schema and `ts`/`seq` unchanged. The batch runs five repetitions of each
E-Swap-3 arm at 1,000 msg/s (20 runs, about an hour). Each eKuiper run first
refuses a package other than 2.1.5 or any rule other than the seeded
`pipeline_a`, and fails if a REST call fails or the updated or replacement
rule does not count output within 10 seconds.

The command exits non-zero when a run failed for the harness; the batch's
`failures.json` lists those runs. A run in which the `kuiper` unit restarted
or the final rule stopped or reported an error is a system outcome instead:
the command exits 0, prints an `OUTCOME` line (`runtime-exit` or
`rule-error`), and the run's `canonical-status.json` records it. Investigate
either before running a batch on that host.

## Smoke-run the E-Swap-3 arms

Before the diagnostic batch, and again before the final batch, run every
E-Swap-3 arm once from the deployed commit:

```sh
./eval/scripts/run-rpi5-canonical.sh --execute --experiments e-swap-3 \
  --repetitions 1 --batch-id swap3-smoke-<date>
```

That is four runs (`wafer-hotswap`, `wafer-restart`, `ekuiper-rule-update`
and `ekuiper-make-before-break`), about ten minutes. It exercises what only
one arm uses: the hot swap to `threshold-filter-v2` (`swap_requests.json`),
the WAFER restart, and the eKuiper REST calls (`rule-update.json`,
`rule-replacement.json`). A `wafer-hotswap` run fails before warm-up when
`threshold-filter-v2` is not built on the host; build the plugins with
`mise run //plugins:build-plugins` (for the Pi, on the development machine,
then deploy again) and run it again.

## Run the host's capacity scout

E-Perf-10 adds a few rates per host beside its common grid, taken from that
host's own capacity scout. Run the scout from the deployed commit before the
diagnostic batch:

```sh
./eval/scripts/run-rpi5-canonical.sh --execute --host rpi5 \
  --capacity-scout --batch-id scout-<date>
```

Each invocation carries out the next probe decision and exits. Repeat it until
it prints `"action": "stop"` and writes
`manifests/capacity-scout/<host>-scout-<date>/scout-complete.json`. Then
preview what the final batch will add:

```sh
./eval/scripts/run-rpi5-canonical.sh --dry-run --host rpi5 \
  --batch-id preview-only --scout-batch-id scout-<date>
```

The `BRACKETS` line lists the bracket rates, the scout summary and its
SHA-256, and the runs and nominal hours the rates add.
[Per-host bracket rates](../../eval/RESULT-CONTRACT.md#per-host-bracket-rates)
defines the rule. The scout's `wafer-max-inflight-1` arm, which runs WAFER with
one MQTT publish in flight, only tests whether eKuiper's one-at-a-time QoS 1
sink explains a capacity gap; it never feeds the E-Perf-10 grid.

## Run a diagnostic batch

Run the first three repetitions of every experiment from the deployed commit:

```sh
./eval/scripts/run-rpi5-canonical.sh --execute --batch-id diag-<date> \
  --seed 1729 --scout-batch-id scout-<date> --repetitions 3
```

Review at least E-Val-1, all four E-Perf-7 modes, every E-Perf-10 system and
rate, all four E-Swap-3 arms with the placebo dip next to each dip, and the
E-Swap-4 burst. Stop and keep the failed attempt if metering, capacity
counters, event alignment, E-Swap-4 phase or drain boundaries, provenance or
throttling is wrong; fix it in a new commit, deploy it, and run a new
diagnostic batch.

Also check resume: stop the batch with SIGINT after a leaf, rerun the same
command, and confirm with
`./eval/scripts/run-rpi5-canonical.sh --status --batch-id diag-<date>` that
admitted attempts are skipped rather than repeated.

## Run or resume the final batch

After a clean diagnostic batch and a clean E-Swap-3 smoke run from the same
deployed commit:

```sh
./eval/scripts/run-rpi5-canonical.sh --execute --batch-id <batch-id> \
  --seed 1729 --scout-batch-id scout-<date>
```

`./eval/scripts/run-rpi5-canonical.sh --status --batch-id <batch-id>` lists
done, pending and missing runs.

On the first start the runner writes `batch.json` into the batch ledger: batch
ID, host, source SHA and dirty flag, SHA-256 of `eval/canonical-matrix.json`,
seed, experiments, repetition override, whether the batch can become thesis
evidence, start time, and `capacity_brackets` (the scout batch, the SHA-256 of
its `scout-complete.json`, and the bracket rates and runs it adds). It also
copies `scout-complete.json` into the ledger; a resume derives the rates from
that copy, so running the scout again does not affect a started batch.

The runner executes sequentially and appends to `progress.jsonl`. Each attempt
ends as a clean pass, a system outcome or an infrastructure failure, recorded
in its `canonical-status.json`;
[Attempts and retries](../../eval/RESULT-CONTRACT.md#attempts-and-retries)
defines them. A clean pass or a system outcome (a runtime exit, a containment
escape) is admitted and never retried. An infrastructure failure is retried
once, in a new attempt directory; if the retry also fails, the run stays
missing. E-Val-1 is never retried: any failed or interrupted repetition fails
the gate, and a new gate needs a new batch. Re-running the same command with
the same batch ID resumes the batch; an interrupted attempt counts as one
infrastructure attempt. A batch that `approve-batch` has sealed cannot be
resumed.

## Monitor and stop safely

Watch the ledger without touching result files:

```sh
tail -f "$WAFER_RESULTS_ROOT/manifests/canonical-batches/rpi5-<batch-id>/progress.jsonl"
find "$WAFER_RESULTS_ROOT/raw" -path "*rpi5-<batch-id>*" -name canonical-status.json -print
```

Stop the runner with SIGINT and do not delete partial attempts. A threshold
miss, crash or containment escape by the system under test is data: the
runner records it and moves on, and it is not a reason to rerun or retune.
Stop the batch instead for thermal throttling or missing telemetry,
provenance, config, binary or plugin drift, low free space, repeated harness
failures, counter or schema mismatches, missed E-Swap event alignment, or
unexpected concurrent activity on the SUT CPUs.

## Bring the host results together

Approval and analysis read every host's batch from one results root. On the
analysis machine, mount or bind-mount one root and copy each host's `raw/` and
`manifests/` trees into it, keeping their relative paths. Batch, scout and
alias directories carry the host tag, so the hosts do not collide. Copy files,
not links: the layout rejects symlinks, hardlinked evidence and case-only name
collisions. Set `WAFER_RESULTS_ROOT` to that root for the steps below.

Analysis opens `raw/` read-only and writes only to `derived/` and `reports/`.
Raw artifacts stay in the results roots, not in the repository.

## Verify contracts

Verify each experiment of the batch:

```sh
python3 eval/scripts/verify-result-contract.py --canonical \
  "$WAFER_RESULTS_ROOT/raw/<experiment>/rpi5-<batch-id>"
```

## Approve the finished batch

On the analysis machine, with the combined results root mounted and the
repository at a commit with the same `eval/canonical-matrix.json`:

```sh
export WAFER_RESULTS_ROOT=<combined results root>
mise run approve-batch -- --batch-id <batch-id> --host rpi5
```

It refuses the batch unless all of these hold:

- the ledger is under `manifests/canonical-batches/`, and its `batch.json`
  marks thesis evidence with no repetition override;
- the batch ran from one clean commit with the current matrix;
- the bracket rates in `batch.json` still follow from the ledger's copy of the
  scout summary;
- `schedule.json` is the full schedule for the batch seed and bracket rates,
  and every run has an admitted attempt within its retry cap, or an alias
  receipt;
- `e-val-1-gate.json` reports a pass;
- every admitted leaf has the batch SHA, a clean tree, `throttled=0x0`, and no
  `thesis_evidence=false`.

It then writes `raw.sha256` into the ledger, listing the SHA-256 of every file
of the batch under `raw/`, its alias receipts and its ledger, with paths
relative to the results root. Check it at any time:

```sh
cd "$WAFER_RESULTS_ROOT"
sha256sum -c manifests/canonical-batches/rpi5-<batch-id>/raw.sha256   # shasum -a 256 -c on macOS
```

Finally it records the batch as the host's entry in `eval/final-batches.json`:
batch ID, WAFER SHA, matrix SHA-256, SHA-256 of `raw.sha256`, approval time,
and `other_final_batches`, which lists any other thesis-eligible batch of the
same host in the results root. Approving the same batch again changes
nothing. To replace a host's approved batch, remove its entry by hand first;
`approve-batch` refuses to overwrite an entry that names another batch.
Commit `eval/final-batches.json`.

## Regenerate the analysis

```sh
export WAFER_RESULTS_ROOT=<combined results root>
mise run //eval:figures <batch-id>
```

The task executes every notebook under `eval/analysis/notebooks/` in place
and writes figures and tables to `eval/analysis/figures/`. To run one notebook
without rewriting it:

```sh
cd eval/analysis
uv sync
export WAFER_EVAL_BATCH_ID=<batch-id>      # jetson-<batch-id> or x86-<batch-id> for those hosts
export WAFER_ANALYSIS_OUTPUT_DIR=figures/final-<batch-id>
uv run jupyter nbconvert --execute --to notebook --output-dir executed \
  notebooks/09-saturation.ipynb
```

Canonical analysis accepts only the batch recorded for its host in
`eval/final-batches.json`;
[the notebooks README](../../eval/analysis/notebooks/README.md#canonical-batch-resolution)
lists every check it makes.
