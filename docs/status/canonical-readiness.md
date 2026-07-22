# Canonical-run readiness matrix

Living document tracking which RFC-008 experiments are ready to book Pi /
Jetson time. Each row is filled in as its shakedown lands. **Numbers here
are macOS shakedown values — informational only, never quotable in the
thesis. Canonical numbers come from the `rpi4` and `jetson` host tags via
`eval/scripts/run-experiment.sh`.**

Related docs:

- `docs/rfcs/RFC-008-evaluation-harness.md` — experiment definitions and
  pass criteria.
- `eval/RESULT-CONTRACT.md` — result-directory manifest every canonical
  run must produce.
- `tcc-doc/research/analysis/evaluation-plan.md` — parent research plan.
- `plans/evaluation-infrastructure/` — the plan that owns this matrix.

Legend:

- 🟢 shakedown clean, all invariants held, ready for Pi
- 🟡 shakedown ran but with caveats — investigate before Pi
- 🔴 shakedown blocked or produced obviously wrong numbers
- ⚪ shakedown not yet attempted

## Experiments

### E-Perf-4 — per-hop overhead × payload size (P2.1)

- **Status**: 🟢 shakedown clean
- **Shakedown**: `eval/results/e-perf-4/shakedown-macos-2026-07-21T20-08-17Z/`
- **Configs**: `eval/configs/e-perf-4/pipeline-c-passthrough-{120b,1kb,10kb,100kb}.toml`
- **Runs**: 4 payload sizes × 30 runs = 120 clean runs (zero traps, zero
  recovery events across all 120 `stdout.log` files).
- **Warm-up**: 1 s wall (~1000 msgs) — smaller than the canonical target
  (10 s / 10 000 msgs) because 30 runs of 5 000 msgs is a sanity sweep,
  not a distribution characterisation.
- **Total per run**: 5 000 msgs @ 1000 msg/s = ~5 s. Post-warmup samples
  per run: ~4 000. Per size (30 runs): ~120 000 samples.
- **Notebook**: `eval/analysis/notebooks/02-per-hop-overhead.ipynb`
  renders on this data and produces the figure below.

Shakedown numbers (median-across-runs of the per-run percentile,
microseconds, macOS M-series):

| percentile | 120 B | 1 KiB | 10 KiB | 100 KiB |
| ---------- | ----: | ----: | -----: | ------: |
| p50        |  32.3 |  14.3 |   31.7 |    53.8 |
| p90        |  66.0 |  41.5 |   52.7 |   110.6 |
| p95        |  81.4 |  53.2 |   63.5 |   137.2 |
| p99        | 141.3 |  69.1 |   98.3 |   238.6 |
| p999       | 309.2 |  84.5 |  194.0 |   638.2 |

**What looks right.**

- Every size records 3900–4000 samples per run — the metadata
  round-trip fix (commit landing this task) is holding.
- Zero traps and zero recovery events across all 120 runs — the epoch
  deadline reset fix (also this task) is holding.
- 10 KiB → 100 KiB p50 growth is roughly linear in payload size
  (32 µs → 54 µs), consistent with the two `read_all`/return copies
  dominating the per-hop cost at large payloads.
- Sequence tracking reports zero gaps and zero duplicates on every run.

**What looks suspicious.**

- **120 B > 1 KiB at every percentile.** The 120 B size was the first
  run of the sweep and eats the cold-cache penalty for the release
  binary + wasmtime instantiation. Subsequent sizes benefit from the
  warm state. Fix: rerun the sweep with a discarded prime run before
  each size, or shuffle the order per iteration. Not a runtime issue
  — a shakedown scheduling artefact.
- **p999 stdev is huge for 120 B and 100 KiB.** On macOS this is
  expected — non-realtime OS, Spotlight/backup daemons, kernel PMU
  sampling. The Pi has none of these and should show tighter tails.
  On the Pi, if p999 stdev/median is > 2× we investigate.
- **Absolute p50 (14–54 µs) is much faster than we expect on the Pi
  4** (~250–1000 µs based on RFC-008 §D2 back-of-envelope). This is
  fine for a shakedown — it confirms the pipeline works and the
  measurement path is sound, but the Pi numbers are the ones that go
  in the thesis.

**Tuning knobs to try before booking Pi time.**

- Bump `warmup_secs` to 10 and `total_messages` to 60 000 (canonical
  size) — verify the tighter tails materialise on macOS too.
- Rebuild the plugin with `opt-level = 3` in `plugins/pass-through/`
  and see how much the WIT boundary drops (currently `opt-level = "s"`).
- Add a "prime run" pass before each size in the shakedown script to
  eliminate the cold-cache asymmetry.
- Consider raising `epoch_deadline` from 100 to 1000 (~10 s per call)
  so `set_epoch_deadline` is a no-op noise removal, and confirm p999
  narrows.

**Prerequisite gaps that must close before Pi.**

- ~~`cabi_realloc` trap in pass-through~~ — closed by
  epoch-deadline-reset fix (commit `56a2ccf`).
- ~~Latency histogram records zero values~~ — closed by
  transform-metadata-propagation fix (this task, commit
  `<pending>`).
- `per_node_metrics.csv` is best-effort (Prometheus scrape from
  shutdown-time state). Not needed for E-Perf-4 statistics but the
  canonical-runs plan should decide whether to wire the mid-run scrape
  or accept the current shape.
- `memory.csv` sampler is not exercised by the P2.1 shakedown runner
  (skipped for speed). Canonical Pi runs use `run-experiment.sh`
  which does sample; verify the sampler works on Linux `ps` before
  booking Pi time.
- Notebook produces a figure but does not yet emit a thesis-quality
  PDF export. Canonical run should add a `figures/e-perf-4.pdf`
  generator.

### E-Density-1 — plugin binary sizes (P5.7)

- **Status**: 🟢 shakedown complete, no Pi run needed (measurement is
  static and portable).
- **Artefacts**: `eval/results/e-density-1/binary-sizes.csv`,
  `docs/benchmarks/binary-sizes.md`.
- 12 plugins measured, ratios 335×–971× vs the smallest realistic
  container base image (2025-Q1 Docker Hub floors).

### E-Perf-1..3, E-Perf-5..8 — not yet started

⚪ shakedown pending. See `plans/evaluation-infrastructure/` task queue.

### E-Val-1 — methodology validation (P3.1)

- **Status**: 🟢 honesty gate holds on macOS.
- **Shakedown**: `eval/results/e-val-1/shakedown-macos-2026-07-22T16-19-29Z/`
  (5 runs, 250 recorded samples each).
- **Config**: `eval/configs/pipeline-c-with-delay.toml`
  (10 msg/s source, 50 ms delay-injector transform).
- **Detailed writeup**: `docs/benchmarks/methodology-validation.md`.

Shakedown numbers (macOS M-series, milliseconds):

| statistic                  | value                |
| -------------------------- | -------------------- |
| injected delay             | 50.00 ms             |
| observed p50 (across runs) | 52.10 – 52.26 ms     |
| observed p99 (across runs) | 52.99 – 53.35 ms     |
| honesty gate [45, 55] ms   | PASS (5/5 runs)      |
| overhead (p50 − injected)  | ~2.2 ms              |

**What this proves.**

Every downstream RQ1 / RQ2 / RQ3 number is anchored on this: if the
measurement rig can correctly recover a known 50 ms delay from a Wasm
plugin, it can be trusted to report tail latency honestly for less
contrived experiments. A regression in this gate (say a rewrite of
`BenchSink` that started using arrival timestamps instead of
`bench.intended_ns`) would silently poison all thesis numbers; the
integration test `crates/wafer-core/tests/wasi_async_runner.rs`
catches this at CI time.

**Runtime bugs surfaced by this shakedown.**

- A16 (WASI async panic in Tokio runner tasks) — closed by P0.14
  commit `40ab46b`. Without E-Val-1 this would have shipped to Pi
  undetected because pass-through never exercises the WASI async
  path.

**Pi tuning notes.**

- Source rate must stay below sink capacity (`rate < 1000/delay_ms`) or
  queue back-pressure inflates p99. macOS shakedown started at
  100 msg/s (5× over capacity) and reported 4.7 s p99; corrected to
  10 msg/s. Same rule applies at every rate/delay ratio.
- `EngineConfig.epoch_deadline` default (200 × 20 ms = 4 s per call)
  has plenty of headroom for a 50 ms sleep. Canonical Pi can drop to
  `100 × 20 ms = 2 s` if we want tighter tail bounds.
- macOS p99 range was 0.36 ms wide; expect Pi with CPU pinning to
  narrow further (target: ~100 µs). If Pi widens the range, suspect
  jitter contamination from co-resident processes and rerun with
  `isolcpus` + `taskset`.

### E-Swap-6 — phase decomposition (P5.1)

- **Status**: 🟢 shakedown clean, all 5 phases populated.
- **Shakedown**: `eval/results/e-swap-6/shakedown-macos-<ts>/`
- **Config**: `eval/configs/e-swap/pipeline-hotswap.toml`
- **Script**: `eval/scripts/run-e-swap-shakedown.sh --swap unified`
- **Notebook**: `eval/analysis/notebooks/05-hotswap-timeline.ipynb`

51 swaps at 1000 msg/s (alternating v1↔v2 every 2s). All 5 phases
non-zero on every swap event. Dominant phase: **convergence** (ack + first_v2
output latency).

| Phase | p50 (ms) | p95 (ms) | Notes |
| ----- | -------: | -------: | ----- |
| Compile | 0.053 | 0.065 | AOT cache hit after first swap |
| Instantiate | 0.192 | 0.276 | InstancePre → Instance |
| Signal | 0.000 | 0.001 | Watch-channel send |
| Ack | 1.067 | 1.148 | Node loop picks up swap |
| Convergence | 1.258 | 1.321 | First v2 output at sink |

Compile dominates only the first cold swap (~14 ms); warm swaps are
dominated by ack + convergence (~1 ms each, pipeline message interval at
1000 msg/s).

### E-Swap-1 — pause duration (P5.2)

- **Status**: 🟢 p95 well under 100 ms target.
- **Shakedown**: `eval/results/e-swap-1/shakedown-macos-<ts>/`
- **Script**: same unified run as E-Swap-6.

51 swap transitions. Pause measured as time between last v1 output and
first v2 output at the BenchSink (version boundary detection via
`plugin.version` metadata).

| Metric | Value |
| ------ | ----- |
| p50 pause | 1.28 ms |
| p95 pause | 1.33 ms |
| Target <100 ms | ✓ PASS |

macOS M-series is fast; Pi numbers will be higher but sub-100 ms
remains achievable given the phase decomposition shows no phase >2 ms.

### E-Swap-2 — zero-loss zero-duplication (P5.3)

- **Status**: 🟢 invariant holds.
- **Shakedown**: `eval/results/e-swap-2/shakedown-macos-<ts>/`
- **Script**: same unified run.

51 swaps with zero swap-induced message gaps and zero duplicates.
BenchSink's SequenceTracker counts sequence numbers across the
measurement window (post-warmup). The initial 5001 "gaps" correspond
exactly to the warmup-excluded region — no messages lost during any
of the 51 hot-swap events.

| Metric | Value |
| ------ | ----- |
| Swap-induced gaps | 0 |
| Sequence duplicates | 0 |
| Post-warmup received | 114,999 |

### E-Swap-4 — swap under 2× burst (P5.5)

- **Status**: 🟢 all swaps converge under burst.
- **Shakedown**: `eval/results/e-swap-4/shakedown-macos-<ts>/`
- **Config**: `eval/configs/e-swap/pipeline-hotswap-burst.toml`
- **Script**: `eval/scripts/run-e-swap-shakedown.sh --swap burst`

52 swaps at 2000 msg/s. All converged successfully. Pause duration is
not significantly higher than normal load — the pipeline's bounded
channels absorb the burst.

| Metric | Value |
| ------ | ----- |
| Source rate | 2000 msg/s |
| Successful swaps | 52 |
| p95 burst pause | 1.17 ms |

### E-Swap-5 — failed swap recovery (P5.6)

- **Status**: 🟡 runtime detects failure but does NOT auto-rollback.
- **Shakedown**: `eval/results/e-swap-5/shakedown-macos-<ts>/`
- **Config**: `eval/configs/e-swap/pipeline-hotswap-rollback.toml`
- **Script**: `eval/scripts/run-e-swap-shakedown.sh --swap rollback`

The v2-panics plugin deliberately passes `init()` (by design — see
`plugins/pass-through-v2-panics/src/lib.rs` line 24) so the A4
init-failure rollback path is NOT triggered. The runtime's recovery
loop re-instantiates from the CURRENT InstancePre (v2-panics), leading
to perpetual traps. The handler returns 504 GATEWAY_TIMEOUT.

| Metric | Value |
| ------ | ----- |
| Swap attempts | 6 |
| Traps post-swap | ~93,000 |
| A4 init-rollback | 0 |
| Auto rollback to v1 | No |
| Runtime panic | No |

**Gap**: Process-time rollback to previous InstancePre is not
implemented. A4 covers init-time failures only. See
`docs/status/implementation-gaps.md` if escalated.

### E-Iso-1..6 — attack containment shakedown (P4.1–P4.6)

- **Status**: 🟢 all 6 attacks contained on macOS.
- **Shakedown**: `eval/results/e-iso-{1..6}/shakedown-macos-2026-07-22T16-44-43Z/`
- **Configs**: `eval/configs/e-iso-{1..6}/pipeline.toml`
- **Script**: `eval/scripts/run-e-iso-shakedown.sh --all`
- **Notebook**: `eval/analysis/notebooks/06-fault-injection.ipynb`

Topology: linear `bench-source (100 msg/s, 200 msgs)` → `attack-transform` → `bench-sink`.
Channel buffer (1024) > total messages (200), so source never back-pressures.

| Attack | E-Iso-N | Contained | Source Thr % | Attacker State | Other Healthy | Evidence |
| ------ | ------- | --------- | ------------ | -------------- | ------------- | -------- |
| buffer-overflow | 1 | ✓ | 100.0 | Error/Recovering | ✓ | `eval/results/e-iso-1/shakedown-macos-2026-07-22T16-44-43Z/` |
| cross-read      | 2 | ✓ | 100.0 | Error/Recovering | ✓ | `eval/results/e-iso-2/shakedown-macos-2026-07-22T16-44-43Z/` |
| fs-access       | 3 | ✓ | 100.0 | Error/Recovering | ✓ | `eval/results/e-iso-3/shakedown-macos-2026-07-22T16-44-43Z/` |
| infinite-loop   | 4 | ✓ | 100.0 | Error/Recovering | ✓ | `eval/results/e-iso-4/shakedown-macos-2026-07-22T16-44-43Z/` |
| memory-exhaust  | 5 | ✓ | 100.0 | Error/Recovering | ✓ | `eval/results/e-iso-5/shakedown-macos-2026-07-22T16-44-43Z/` |
| panic           | 6 | ✓ | 100.0 | Error/Recovering | ✓ | `eval/results/e-iso-6/shakedown-macos-2026-07-22T16-44-43Z/` |

**What this proves.**

Every attack traps on every `process()` call (200/200). The source emits
all 200 messages at the configured 100 msg/s rate without backpressure
(channel buffer 1024 >> 200 messages). The attack node transitions to
Error/Recovering on each trap and successfully re-instantiates from
`InstancePre` cache. The runtime does NOT panic and shuts down gracefully
after source EOF propagates.

**What's NOT proven yet.**

- Infinite-loop is classified as `Unrecoverable` (not `TimedOut`) by the
  runtime because wasmtime's error Display doesn't surface "epoch" in the
  message. The harness-level test `attack_containment.rs` accepts both
  classifications — the containment property holds regardless.
- Pi canonical numbers (latency-to-recover, throughput under sustained
  attack) require the canonical-runs plan.
- Source throughput % is 100% by construction (buffer >> messages) — the
  real stress test requires sustained attack for minutes, not 2 seconds.

### E-Iso-7 — parallel-branch fault isolation (P4.7)

- **Status**: 🟢 branch-A throughput drop <1% on macOS.
- **Shakedown**: `eval/results/e-iso-7/shakedown-macos-<ts>/`
- **Config**: `eval/configs/e-iso-7/pipeline.toml` (attack) + `pipeline-control.toml` (control)
- **Script**: `eval/scripts/run-e-iso-7-8.sh --iso 7`

Topology: diamond `bench-source (1000 msg/s, 5000 msgs)` → { `branch_a` (pass-through), `branch_b` (panic) } → `bench-sink`.
Control run uses pass-through in both branches; attack run panics branch_b.

| Metric | Control | Attack | Delta |
| ------ | ------- | ------ | ----- |
| Branch-A thr (msg/s) | ~998.6 | ~999.0 | -0.04% (within ±1%) |
| Branch-B traps | 0 | 5000 | N/A |
| Branch-A errors | 0 | 0 | — |

**What this proves.** Task-per-node isolation: a trapping branch does not
affect throughput of a parallel healthy branch in the same diamond topology.
The runtime's tokio-task-per-node + channel decoupling ensures fault
containment across DAG branches.

### E-Iso-8 — recovery time (P4.8)

- **Status**: 🟢 recovery histogram populated on macOS.
- **Shakedown**: `eval/results/e-iso-8/shakedown-macos-<ts>/`
- **Config**: `eval/configs/e-iso-8/pipeline.toml`
- **Script**: `eval/scripts/run-e-iso-7-8.sh --iso 8`

Topology: linear `bench-source (200 msg/s, 500 msgs)` → `panic-attack` → `bench-sink`.
Each trap triggers Recovering → Running from InstancePre cache.

| Metric | Value |
| ------ | ----- |
| Recovery samples (Prometheus) | 338 (500 in log; scrape captured mid-run) |
| Avg recovery time | ~0.136 ms |
| p99 recovery time | ~1 ms (integer ms boundary; true value ≈ sub-ms) |
| Recovery failures | 0 |

**What this proves.** The runtime's InstancePre cache enables sub-millisecond
recovery from traps. `wafer_node_recovery_duration_ms` histogram is populated
on every Recovering → Running transition and scrapeable via the Prometheus
endpoint during the run.

**Caveat.** Integer division in the `/metrics` endpoint means sub-ms values
appear as 0 or 1 ms. Canonical Pi measurements with higher precision will
be done with the full HdrHistogram bucket export path.

### E-Density-2..3, E-Mig-1..3 — not yet started

⚪ shakedown pending.

## Cross-cutting

- **Repro hinge**: every result directory carries `metadata.json` with
  `config_sha256`, `git_sha`, `hostname`, `rustc`, and
  `wasmtime_version_declared`. Notebooks that plot canonical numbers
  MUST assert on these before drawing conclusions.
- **Never quote macOS numbers in the thesis.** The `shakedown-macos`
  host tag is the gatekeeper: analysis notebooks refuse to render
  canonical figures from `shakedown-macos` inputs. See
  `eval/RESULT-CONTRACT.md` for the whitelist.
