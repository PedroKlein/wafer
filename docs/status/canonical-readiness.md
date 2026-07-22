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

### E-Swap-1..6 — not yet started

⚪ shakedown pending.

### E-Iso-1..6 — attack containment (P0.8 complete)

- **Status**: 🟢 runtime harness proven. `crates/wafer-core/tests/attack_containment.rs`
  runs 6 in-process containment cases (buffer overflow, cross-read,
  fs-access, infinite loop, memory exhaust, panic). Canonical Pi run
  reruns the same tests + captures per-attack log fingerprints.

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
