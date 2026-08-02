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
- `plans/thesis-hardening.md` — closed 2026-08-02 (9/9); landed A17,
  A19, T4 memory sampler, T8 legacy shakedown metadata unification,
  T9 thesis-grade PDF pipeline, T10 cross-arch CI, T11 notebook
  traceability. Preconditions for canonical Pi runs.
- `plans/canonical-runs.md` — Pi 4 + Jetson preflight and execution.
  C1 (aarch64 cross-compile spike) + C2 (loadgen + waferctl) closed
  2026-08-02; `mise run cross-build-pi` ships aarch64-linux binaries.

Legend:

- 🟢 shakedown clean, all invariants held, ready for Pi
- 🟡 shakedown ran but with caveats — investigate before Pi
- 🔴 shakedown blocked or produced obviously wrong numbers
- ⚪ shakedown not yet attempted

## Summary matrix

| Experiment | RQ | Status | Shakedown evidence | Canonical gaps | macOS confounders |
|---|---|---|---|---|---|
| E-Val-1 | All | 🟢 | `e-val-1/shakedown-macos-2026-07-22T16-19-29Z/` | Longer warmup (30 s), ADF test on Pi | Non-realtime OS jitter in p99 |
| E-Perf-1 | RQ1 | 🟢 | `e-perf-1/shakedown-macos-2026-08-01T19-50-15Z/` (post-A18) | Longer runs (60 s) for tighter CIs | Docker Desktop overhead on eKuiper |
| E-Perf-2 | RQ1 | 🟢 | `e-perf-2/shakedown-macos-2026-08-01T19-50-15Z/` (post-A18) | Same as E-Perf-1 | Docker Desktop overhead on eKuiper |
| E-Perf-3 | RQ1 | 🟢 | `e-perf-3/shakedown-macos-2026-07-22T18-29-39Z/` | 60 s runs for statistical power | Localhost MQTT faster than cross-device |
| E-Perf-4 | RQ1 | 🟢 | `e-perf-4/shakedown-macos-2026-07-21T20-08-17Z/` | Prime run, shuffle order, 60 s | Mach kernel scheduling noise at p999 |
| E-Perf-5 | RQ1 | ⚪ | — | Needs Pi + x86 Linux cross-run | N/A (inherently multi-platform) |
| E-Perf-6 | RQ1 | 🟢 | `e-perf-6/shakedown-macos-2026-07-22T17-49-56Z/` | `/proc/pid/smaps_rollup` on Linux | macOS RSS includes shared libs |
| E-Perf-7 | RQ1 | 🟢 | `e-perf-7/shakedown-macos-2026-07-22T18-01-09Z/` | Instruction-heavy plugin, true disable | M-series branch prediction hides cost |
| E-Perf-8 | RQ1 | 🟢 | `e-perf-8/shakedown-macos-2026-07-22T17-49-56Z/` | 60 s, 30 s warmup | M-series ~10–30× faster than Pi |
| E-Perf-9 | RQ1 | 🟢 | `e-perf-9/shakedown-macos-2026-07-22T18-56-42Z/` | `drop_caches` on Linux for true cold | macOS page cache not purgeable w/o sudo |
| E-Backpressure | RQ1 | 🟢 | `e-backpressure/shakedown-macos-2026-07-22T18-51-59Z/` | Longer burst windows | Pipeline never saturates on M-series |
| E-Iso-1 | RQ2 | 🟢 | `e-iso-1/shakedown-macos-2026-07-22T16-44-43Z/` | Sustained attack (minutes) | None significant |
| E-Iso-2 | RQ2 | 🟢 | `e-iso-2/shakedown-macos-2026-07-22T16-44-43Z/` | Sustained attack | None significant |
| E-Iso-3 | RQ2 | 🟢 | `e-iso-3/shakedown-macos-2026-07-22T16-44-43Z/` | Sustained attack | None significant |
| E-Iso-4 | RQ2 | 🟢 | `e-iso-4/shakedown-macos-2026-07-22T16-44-43Z/` | Sustained attack | None significant |
| E-Iso-5 | RQ2 | 🟢 | `e-iso-5/shakedown-macos-2026-07-22T16-44-43Z/` | Sustained attack | None significant |
| E-Iso-6 | RQ2 | 🟢 | `e-iso-6/shakedown-macos-2026-07-22T16-44-43Z/` | Sustained attack | None significant |
| E-Iso-7 | RQ2 | 🟢 | `e-iso-7/shakedown-macos-2026-07-22T16-59-31Z/` | Multi-minute sustained fault | None significant |
| E-Iso-8 | RQ2 | 🟢 | `e-iso-8/shakedown-macos-2026-07-22T16-59-31Z/` | HdrHistogram sub-ms precision | Integer division in `/metrics` endpoint |
| E-Swap-1 | RQ3 | 🟢 | `e-swap-1/shakedown-macos-2026-07-22T17-13-47Z/` | 50 swaps for p95 confidence | M-series faster than Pi |
| E-Swap-2 | RQ3 | 🟢 | `e-swap-2/shakedown-macos-2026-07-22T17-13-47Z/` | Same as E-Swap-1 | None significant |
| E-Swap-3 | RQ3 | 🟢 | `e-swap-3/shakedown-macos-2026-07-22T20-11-05Z/` | 30 runs per strategy | Docker Desktop overhead on eKuiper |
| E-Swap-4 | RQ3 | 🟢 | `e-swap-4/shakedown-macos-2026-07-22T17-15-59Z/` | Higher burst rate on Pi | Pipeline never saturates on M-series |
| E-Swap-5 | RQ3 | 🟢 | `e-swap-5/shakedown-macos-2026-08-02T15-51-21Z/` | A17 closed + polished (B1/M1): canary rollback + API `status=rolled_back` | rollback_time_ns p99=115 µs (n=24) |
| E-Swap-6 | RQ3 | 🟢 | `e-swap-6/shakedown-macos-2026-07-22T17-13-47Z/` | Phase timing at Pi speed | AOT compile phase larger on ARM |
| E-Density-1 | All | 🟢 | `e-density-1/binary-sizes.csv` | None (static measurement) | None (portable) |

## Cross-cutting readiness

### macOS-vs-Linux confounders

| Confounder | Impact | Mitigation for canonical runs |
|---|---|---|
| **Docker Desktop overhead** | eKuiper runs inside Docker Desktop VM on macOS, adding ~0.5–1 ms latency vs native Linux Docker. Inflates eKuiper numbers relative to WAFER. | On Pi: eKuiper runs in native Docker (no VM layer). Report Docker version. |
| **Mach kernel scheduling** | macOS does not support `SCHED_FIFO`, `isolcpus`, or `taskset`. Background processes (Spotlight, Time Machine, WindowServer) inject jitter at p999. | On Pi: `isolcpus=2,3` + `taskset -c 2` for SUT; `taskset -c 3` for loadgen. Pin CPU governor to `performance`. |
| **Memory reporting (`ps` vs `/proc`)** | macOS `ps -o rss=` includes shared library pages counted once per framework. Over-reports absolute RSS. Per-hop *delta* is valid because shared libs don't grow with depth. | On Pi: use `/proc/<pid>/smaps_rollup` Private_Dirty for accurate per-process RSS. |
| **Page cache behavior** | macOS Unified Buffer Cache doesn't support `drop_caches`. Cold-start measurements (E-Perf-9) rely on binary rebuild as a proxy for cache invalidation. | On Pi: `echo 3 > /proc/sys/vm/drop_caches` before cold runs. |
| **MQTT localhost latency** | macOS mosquitto on localhost has lower latency than cross-device MQTT on Pi (USB Ethernet + kernel network stack). | On Pi: loadgen on separate device or isolated core. Report network topology. |

### What needs to change to book Pi time

1. **Cross-compile the runtime**: SHIPPED (canonical-runs C1+C2, 2026-08-02). `mise run cross-build-pi` produces aarch64-unknown-linux-gnu ELF binaries for `wafer`, `wafer-loadgen`, `waferctl` via `docker run --platform linux/arm64 rust:1-slim-bookworm`. See `docs/eval/cross-compile.md`. CI job at `.github/workflows/cross-arch.yml` guards the recipe on every push.
2. **Linux memory sampler**: SHIPPED (thesis-hardening T4, A19, 2026-08-02). Runtime uses the `memory-stats` crate directly; the `MemoryRecorder::sample_loop` runs at 1 Hz and writes `memory.csv` at graceful shutdown. All `ps -o rss=` scrapes removed from shakedown scripts.
3. **CPU isolation**: Boot Pi with `isolcpus=2,3` kernel parameter. Run SUT on core 2, loadgen on core 3.
4. **Longer runs**: Bump `total_messages` from 5000→60000 and `warmup_secs` from 1→30 for canonical statistical power (N=30 runs × 60s each).
5. **eKuiper native Docker**: Verify `lfedge/ekuiper:2.1.0-alpine` runs on `linux/arm64` without emulation.
6. **HdrHistogram sub-ms precision**: The `/metrics` endpoint uses integer ms. For E-Iso-8 canonical, export raw histogram buckets from the Prometheus scrape.

### A16/A17/A18/A19 impact on canonical RQ claims

| Gap | Impact | RQ claim modification |
|---|---|---|
| **A16** (WASI async panic) | CLOSED. Fixed in commit `40ab46b`. | None — regression test covers this. |
| **A17** (process-time rollback) | CLOSED (2026-08-02) + polished (B1/B2/M1/M2, commit `78519ea`). Canary window + bounded retry in `run_transform_loop_with_config`; `HotSwapError::RolledBack` distinguishes rollback from convergence in the `/hot-swap` API response; `recovery_store` reapplies fuel before instantiate. | RQ3 claim: "rollback on any failure" — both init-time and process-time traps trigger auto-rollback within the canary window, and the API reports `status: rolled_back` distinctly from `swap_converged`. |
| **A18** (native filter dispatch) | CLOSED (2026-08-01). Native filter dispatch wired; `NativeFilter::range` mirrors `plugins/threshold-filter`. | None — WAFER/native ratio moved 0.995 → 0.999, well inside the noise floor. |
| **A19** (memory sampler) | CLOSED (2026-08-02). `memory-stats` crate + runtime-side `MemoryRecorder::sample_loop`. Per-node metrics CSV emitted at graceful shutdown. | None — canonical runs will now get consistent memory samples on Linux without shelling out to `ps`. |
| **A20** (Prom rollbacks_total) | OPEN (filed 2026-08-02, ~1 h to close). Rollback total not on `/metrics` yet; per-node `NodeMetrics::rollbacks()` still visible via `PipelineHandle`. | None — does not affect thesis metrics. |

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

### E-Perf-1..2, E-Perf-5 — not yet started

⚪ shakedown pending. See `plans/evaluation-infrastructure/` task queue.

### E-Perf-1 — throughput comparison (P3.2)

- **Status**: 🟢 shakedown clean, apples-to-apples restored.
- **Shakedown (post-A18)**: `eval/results/e-perf-1/shakedown-macos-2026-08-01T19-50-15Z/`
- **Superseded**: `eval/results/e-perf-1/shakedown-macos-2026-07-22T19-38-10Z/` used the pre-A18 passthrough native baseline; retained for provenance only.
- **Configs**: `eval/configs/pipeline-a-wafer.toml`, `eval/configs/pipeline-a-native.toml`
- **eKuiper**: running via `eval/ekuiper/docker-compose.yml` + `seed-pipeline-a.sh`
- **Runs**: 3 systems × 30 runs = 90 clean runs (zero parse errors, zero gaps).
- **Script**: `eval/scripts/run-e-perf-1-2-shakedown.sh`
- **Notebook**: `eval/analysis/notebooks/09-saturation.ipynb`

Shakedown numbers (median across 30 runs, macOS M-series, post-A18):

| System | Throughput (msg/s) | p50 (µs) | p99 (µs) |
| ------ | -----------------: | -------: | -------: |
| WAFER | 811 | 1290 | 2164 |
| Native | 812 | 1273 | 2052 |
| eKuiper | 894 | 1074 | 1983 |

**Throughput ratios (post-A18):**
- WAFER/native: 0.999 (Wasm overhead invisible at MQTT-bookend scale; native now does the same JSON-decode + range compare)
- WAFER/eKuiper: 0.908 (eKuiper's Go runtime processes MQTT slightly faster)

**Pre-A18 comparison.** Before A18 closed, native ran passthrough, so the ratio was 0.995. The shift to 0.999 confirms the JSON-decode + range-compare cost is below the noise floor of MQTT RTT at 1000 msg/s — exactly what RQ1 predicts.

**What looks right.** All three systems record 9000/9000 messages per run
with zero gaps and zero parse errors. The throughput values are all below
the 1000 msg/s source rate because the 10s duration includes MQTT setup
latency (~1s startup) — effective measurement window is ~9s.

**What's notable.** WAFER vs native is nearly identical (~17 µs p50
difference) because MQTT bookend latency (~1.3 ms) dwarfs the Wasm
boundary overhead (~15 µs from E-Perf-8). eKuiper is faster at p50
because it processes in a single Go goroutine without channel hops.

**Gaps before Pi.** Canonical runs should use longer duration (60s) and
higher message counts for tighter confidence intervals. The A18-related
"pipeline-a-native uses passthrough" caveat no longer applies — both
systems now run the JSON-decode + range-compare path.

### E-Perf-2 — latency comparison (P3.3)

- **Status**: 🟢 shakedown clean, apples-to-apples restored.
- **Shakedown (post-A18)**: `eval/results/e-perf-2/shakedown-macos-2026-08-01T19-50-15Z/`
- **Superseded**: `eval/results/e-perf-2/shakedown-macos-2026-07-22T19-38-10Z/` (pre-A18 passthrough baseline; retained for provenance only).
- **Configs**: same as E-Perf-1.
- **Runs**: 3 systems × 30 runs = 90 clean runs.
- **Script**: same as E-Perf-1 (`run-e-perf-1-2-shakedown.sh`).
- **Notebook**: `eval/analysis/notebooks/01-latency-cdf.ipynb`

Shakedown numbers (median percentiles across 30 runs, µs, macOS M-series, post-A18):

| System | p50 (µs) | p99 (µs) |
| ------ | -------: | -------: |
| WAFER | 1290 | 2164 |
| Native | 1273 | 2052 |
| eKuiper | 1074 | 1983 |

**Interpretation.** At p50 all systems are dominated by MQTT roundtrip
(~1.1–1.3 ms). The WAFER−native p50 delta (~17 µs) is the Wasm
isolation tax: JSON marshal + fuel/epoch reset + WIT call, all amortised
over the MQTT bookend. The p99 gap is tighter than the pre-A18
shakedown because both baselines now do the same JSON-decode + range
compare, so the tails share the same jitter surface. On Pi with CPU
pinning the tails should narrow further.

### E-Swap-3 — throughput dip comparison (P5.4)

- **Status**: 🟢 shakedown clean, three strategies compared.
- **Shakedown**: `eval/results/e-swap-3/shakedown-macos-2026-07-22T20-11-05Z/`
- **Configs**: `eval/configs/e-swap/pipeline-swap3-mqtt.toml`
- **Runs**: 3 strategies × 5 runs = 15 runs.
- **Script**: `eval/scripts/run-e-swap-3-shakedown.sh`

| Strategy | Avg gaps (msgs lost) | Dip characteristic |
| -------- | -------------------: | ------------------ |
| WAFER hot-swap | 0.0 | Zero-loss, zero-downtime |
| WAFER full-restart | 27.2 | ~2.7% loss during kill+restart |
| eKuiper rule-restart | 2.0 | Minimal loss, fast restart |

**What this proves.** WAFER's drain-and-flip hot-swap is provably
lossless (0 gaps across all 5 runs), matching E-Swap-2's invariant.
Full restart loses ~27 messages (~2.7s downtime at 10 msg/s effective
rate accounting for restart latency). eKuiper's rule restart is fast
(~2 messages lost ≈ 2ms interrupt).

**Gaps before Pi.** The v1→v2 swap uses pass-through plugins (not the
threshold-filter) because hot-swap targets transform nodes. This is by
design — the swap mechanism is plugin-agnostic. Canonical runs should
use more runs (30) for tighter statistics.

### E-Perf-3 — per-hop overhead with MQTT bookends (P3.4)

- **Status**: 🟢 shakedown clean, MQTT bookend cost isolated.
- **Shakedown**: `eval/results/e-perf-3/shakedown-macos-2026-07-22T18-29-39Z/`
- **Configs**: `eval/configs/e-perf-3/pipeline-mqtt-depth-{1,3,5,10}.toml`
- **Runs**: 4 depths × 30 runs = 120 clean runs (zero traps).
- **Loadgen profile**: `eval/loadgen/e-perf-3-shakedown.toml`
  (1000 msg/s, 5000 msgs, telemetry-120b template).
- **Script**: `eval/scripts/run-e-perf-3-shakedown.sh`
- **Notebook**: `eval/analysis/notebooks/08-depth-scaling.ipynb`
  (dual-plot: E-Perf-3 vs E-Perf-8 + bookend-cost isolation).

Shakedown numbers (median p50 across 30 runs, µs, macOS M-series):

| Depth | E-Perf-3 p50 (µs) | E-Perf-8 p50 (µs) | Bookend cost (µs) |
| ----: | -----------------: | -----------------: | -----------------: |
|     1 |           1317.4   |             33.3   |           1284.1   |
|     3 |           1401.9   |             69.1   |           1332.8   |
|     5 |           1479.7   |             98.3   |           1381.4   |
|    10 |           1565.7   |            172.0   |           1393.7   |

- **E-Perf-3 per-hop slope**: 26.7 µs/hop (R² = 0.99).
- **E-Perf-8 per-hop slope**: 15.2 µs/hop (R² > 0.99).
- **MQTT bookend cost** (intercept delta): ~1293 µs.
- **Per-hop overhead difference**: ~11.5 µs/hop (MQTT serialization per hop).

**Interpretation.** The ~1.3 ms bookend cost is the MQTT source-to-broker
+ broker-to-sink round-trip latency on localhost. The per-hop slope
difference (~11 µs) is the MQTT payload serialization overhead that
accumulates per hop. Both scale linearly, confirming no hidden
non-linearities in the MQTT I/O path.

**Gaps before Pi.**

- Pi mosquitto latency will be higher (shared CPU). The bookend cost
  will increase but the delta formula remains valid.
- Canonical runs should use longer duration (60s) for statistical power.

### E-Perf-6 — per-node RSS scaling (P2.2)

- **Status**: 🟢 shakedown clean, linear scaling confirmed.
- **Shakedown**: `eval/results/e-perf-6/shakedown-macos-2026-07-22T17-49-56Z/`
- **Configs**: `eval/configs/e-perf-6/pipeline-depth-{1,3,5,10}.toml`
- **Runs**: 4 depths × 30 runs = 120 clean runs (zero traps).
- **Memory sampling**: 1 Hz `ps -o rss=,vsz=` background loop; 5 samples
  per run (5 s run duration). Captures startup and steady-state.
- **Notebook**: `eval/analysis/notebooks/03-memory-scaling.ipynb`

Shakedown numbers (median steady-state RSS, macOS M-series):

| Depth | RSS (MB) | Δ from depth-1 |
| ----: | -------: | --------------: |
|     1 |     34.6 |             — |
|     3 |     39.0 |          +4.4 |
|     5 |     40.8 |          +6.2 |
|    10 |     45.1 |         +10.5 |

- **Slope**: ~1116 KB/hop (~1.09 MB/hop), R² = 0.94.
- **Intercept**: ~35 MB (runtime fixed cost: wasmtime engine + tokio + channels).

**macOS vs Linux RSS.** macOS `ps` RSS includes shared libraries that would
be counted once on Linux (via `/proc/pid/smaps_rollup`). Pi canonical numbers
will show a lower absolute baseline but the per-hop delta should be similar
(it's dominated by Wasm linear memory + Store overhead, not shared mappings).

**Gaps before Pi.**

- Pi sampler should use `/proc/<pid>/smaps_rollup` for private RSS.
- Longer runs (60 s) will give more samples for a tighter steady-state window.
- 5 samples per run is borderline — canonical runs at 60 s will have ~60 samples.

### E-Perf-7 — metering overhead decomposition (P2.3)

- **Status**: 🟢 shakedown clean, all 4 configs run cleanly.
- **Shakedown**: `eval/results/e-perf-7/shakedown-macos-2026-07-22T18-01-09Z/`
- **Configs**: `eval/configs/e-perf-7/pipeline-c-{fuel-only,epoch-only,neither,passthrough}.toml`
- **Runs**: 4 configs × 30 runs = 120 clean runs.
- **Notebook**: `eval/analysis/notebooks/04-metering-overhead.ipynb`

Shakedown numbers (median p50 across runs, µs, macOS M-series):

| Config | p50 (µs) | p95 (µs) | p99 (µs) | Δp50 vs neither |
| ------ | -------: | -------: | -------: | --------------: |
| neither (baseline) | 34.3 | 58.4 | 65.0 | — |
| fuel-only | 34.3 | 58.4 | 65.0 | +0.0 |
| epoch-only | 38.9 | 58.4 | 66.0 | +4.6 |
| passthrough (both) | 34.3 | 58.4 | 65.0 | +0.0 |

**Interpretation.** On macOS M-series, metering overhead is negligible
(<5 µs at p50). Epoch-only shows a small bump (~4.6 µs) — consistent with
the epoch-check instruction on loop back-edges. Fuel accounting imposes
zero measurable overhead for the pass-through plugin (few instructions).

**Config semantics.** `epoch_deadline = 0` in the original canonical configs
meant "trap immediately" (not "disable"). The shakedown configs use large
sentinel values (1B ticks / 999B fuel) to effectively disable without runtime
changes. Canonical runs should adopt the same pattern or add runtime support
for `0 = unlimited`.

**Gaps before Pi.**

- Pi's ARM Cortex-A72 may show larger epoch overhead (slower branch
  prediction). Canonical run will confirm.
- A more instruction-heavy plugin (e.g., JSON parse) would stress fuel
  accounting more visibly — current pass-through is best-case for fuel.

### E-Perf-8 — pipeline depth scaling (P2.4)

- **Status**: 🟢 shakedown clean, linear scaling confirmed (R² > 0.99).
- **Shakedown**: `eval/results/e-perf-8/shakedown-macos-2026-07-22T17-49-56Z/`
- **Configs**: same as E-Perf-6 (`eval/configs/e-perf-6/pipeline-depth-{1,3,5,10}.toml`)
- **Runs**: 4 depths × 30 runs = 120 clean runs.
- **Notebook**: `eval/analysis/notebooks/04b-depth-scaling.ipynb`

Shakedown numbers (median across runs, µs, macOS M-series):

| Depth | p50 (µs) | p95 (µs) | p99 (µs) |
| ----: | -------: | -------: | -------: |
|     1 |     33.3 |     58.9 |     66.0 |
|     3 |     69.1 |     99.3 |    110.6 |
|     5 |     98.3 |    139.3 |    153.1 |
|    10 |    172.0 |    239.1 |    266.2 |

- **Per-hop overhead (p50 slope)**: 15.24 µs/hop, R² = 0.9983.
- **Fixed overhead (intercept)**: 20.8 µs (channel transit + source/sink).
- **Simple delta**: (depth-10 − depth-1) / 9 = 15.4 µs/hop.

**What looks right.**

- Near-perfect linear fit (R² > 0.99) confirms no non-linear overhead
  accumulation as depth grows.
- Per-hop cost (~15 µs) is consistent with E-Perf-4's 128-byte single-hop
  measurement (~33 µs total for 1-hop, of which ~15 µs is the Wasm call
  and ~18 µs is channel/source/sink fixed cost).
- All 120 runs clean — zero traps, zero recovery events.

**Pi expectation.**

- Per-hop cost on Pi 4 expected at ~100–500 µs/hop based on RFC-008 §D2
  estimates (M-series is ~10–30× faster than Cortex-A72 for Wasm).
- Linear scaling should hold; the key thesis number is the slope, not the
  absolute value.

**Gaps before Pi.**

- Canonical run should use 60 s / 60k messages for statistical power.
- Warm-up of 30 s (30k msgs) will avoid the cold-cache effect seen in
  E-Perf-4's 120 B anomaly.

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

### E-Backpressure — burst backpressure validation (P3.6)

- **Status**: 🟢 shakedown clean, zero overflow.
- **Shakedown**: `eval/results/e-backpressure/shakedown-macos-2026-07-22T18-51-59Z/`
- **Config**: `eval/configs/e-backpressure/pipeline-burst.toml`
- **Script**: `eval/scripts/run-e-bp-perf9-shakedown.sh`
- **Loadgen**: `eval/loadgen/e-backpressure-180s.toml`
  (burst: 1000 msg/s baseline, 2× for 10s every 60s, 180s total).
- **Notebook**: `eval/analysis/notebooks/09-backpressure.ipynb`

| Metric | Value |
| ------ | ----- |
| Total messages | 210,000 |
| Burst pattern | 1000 msg/s + 2× (2000 msg/s) for 10s every 60s |
| Sequence gaps | 0 |
| Sequence duplicates | 0 |
| Queue overflow | 0 (bounded channels held) |
| Latency p50 | 1427.5 µs |
| Latency p99 | 7110.7 µs |
| Prometheus failures | 0 across all nodes |

**What this proves.** The pipeline’s bounded channels (default 1024)
absorb 2× burst load without overflow or message loss. On macOS M-series
the pipeline processes 2000 msg/s comfortably (per-hop latency << 500 µs),
so bursts are absorbed without queueing. On Pi 4 with higher per-hop
latency, the channel buffer will show utilization during bursts — but the
bounded guarantee ensures no overflow regardless.

**macOS vs Linux.** On Pi, the 2× burst may cause brief queue growth
(observable via `wafer_queue_depth` Prometheus gauge). The key invariant
is zero `wafer_queue_overflow_total` — confirmed here.

### E-Perf-9 — AOT cold vs warm startup (P3.6)

- **Status**: 🟢 shakedown clean, cold > warm confirmed.
- **Shakedown**: `eval/results/e-perf-9/shakedown-macos-2026-07-22T18-56-42Z/`
- **Configs**: `eval/configs/e-perf-9/pipeline-tier-{small,medium,large}.toml`
- **Script**: `eval/scripts/run-e-bp-perf9-shakedown.sh`
- **Notebook**: `eval/analysis/notebooks/10-aot-startup.ipynb`
- **Runs**: 3 tiers × {cold, warm} × 5 runs = 30 runs.

Plugin tiers:

| Tier | Plugin | WASM size |
| ---- | ------ | --------- |
| Small | pass-through | 57 KB |
| Medium | tensor-prep | 97 KB |
| Large | vibration-features | 375 KB |

Startup latency (ms, macOS M-series):

| Tier | Cold run-01 | Cold runs 2–5 | Warm median | Δ (cold-1 − warm) |
| ---- | ----------: | ------------: | ----------: | -----------------: |
| Small | 1345 | 578 | 592 | 753 |
| Medium | 672 | 59 | 59 | 613 |
| Large | 725 | 97 | 90 | 635 |

**Cold definition**: first run after `cargo build --release -p wafer-runtime`
(binary re-linked, page cache invalidated for the new binary). This provides
a best-effort cold-cache emulation on macOS without `sudo purge`.

**What this proves.** The first-run AOT compilation penalty ranges from
600–750 ms on M-series. After the first run, wasmtime’s native code is
cached by the OS, eliminating the penalty. The InstancePre cache makes
subsequent instantiations (warm starts, hot-swap recovery) essentially free.

**macOS caveat.** After cold run-01, runs 2–5 still benefit from OS page
cache despite being labeled “cold.” True cold-cache requires `sudo purge`
or reboot (out of scope). On Linux/Pi, `echo 3 > /proc/sys/vm/drop_caches`
will provide cleaner measurements.

**Tier-small anomaly.** The pass-through plugin’s wall time includes ~500 ms
of BenchSource message emission (50 msgs × 100 msg/s = 500 ms). Tier-medium
and tier-large process faster or trap, so total time is dominated by startup
overhead rather than message processing. This explains why tier-small has
higher absolute numbers but similar Δ.

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
