# Evaluation Progress

Current progress against the thesis evaluation plan
(`tcc-doc/research/analysis/evaluation-plan.md`). The macOS shakedown pass
is **complete** — all experiments that can run on a single macOS developer
machine have been executed and validated. See
[`canonical-readiness.md`](./canonical-readiness.md) for the per-experiment
gap analysis and what's needed to book Pi/Jetson time.

## Status summary

| Category | Total | Shakedown ✓ | Pending canonical | Blocked |
|----------|------:|------------:|------------------:|--------:|
| E-Val | 1 | 1 | 0 | 0 |
| E-Perf | 9 | 8 | 8 | 1 (E-Perf-5: needs Pi) |
| E-Iso | 8 | 8 | 8 | 0 |
| E-Swap | 6 | 6 | 6 | 0 |
| E-Backpressure | 1 | 1 | 1 | 0 |
| E-Density | 1 | 1 | 0 (static) | 0 |
| **Total** | **26** | **25** | **23** | **1** |

## Shakedown pass (macOS Apple Silicon)

**Completed 2026-07-22.** All 25 executable experiments produced clean
results on macOS. Analysis notebooks (11 canonical + 4 auxiliary) execute
headless against the shakedown data and produce figures/tables.

Key findings from shakedown:
- **RQ1**: WAFER/native throughput ratio = 0.995 (negligible isolation tax at MQTT scale).
  Per-hop overhead = 15.2 µs (linear, R² > 0.99). Memory = 1.1 MB/hop.
- **RQ2**: All 6 attacks contained. Branch-A throughput drop = -0.04%.
  Recovery time = ~0.136 ms (sub-ms from InstancePre cache).
- **RQ3**: Pause p95 = 1.33 ms (target <100 ms). Zero message loss across
  51 swaps. Process-time rollback NOT implemented (A17).

## What's next: canonical runs on Raspberry Pi 4

The canonical-readiness matrix (`docs/status/canonical-readiness.md`)
documents every experiment's gap list, macOS confounders, and what needs to
change before booking Pi time. Key prerequisites:

1. Cross-compile the runtime for `aarch64-unknown-linux-gnu`.
2. Set up Pi with `isolcpus`, `taskset`, CPU governor pinning.
3. Bump run duration from 5s → 60s and warmup from 1s → 30s.
4. Replace macOS `ps` memory sampler with `/proc/pid/smaps_rollup`.
5. Verify eKuiper Docker image on `linux/arm64`.

## Runtime infrastructure

| Component | Status |
|-----------|--------|
| Open-loop load generator (`wafer-loadgen`) | ✅ Fully operational |
| Native Rust baseline (Pipeline D) | ✅ Passthrough functional |
| eKuiper comparison setup | ✅ Docker Compose + seed script |
| Attack scenario Wasm modules (S1–S6) | ✅ All 6 implemented and tested |
| Per-hop latency instrumentation | ✅ HdrHistogram + sequence tracker |
| Per-node RSS sampling | ✅ 1 Hz `ps` reader (macOS); needs `/proc` for Linux |
| Pipeline configs (1/3/5/10-node chains) | ✅ All checked in |
| Analysis notebooks | ✅ 11 canonical + 4 auxiliary |

## E-Val — Methodology validation

| ID | Purpose | Status |
|----|---------|--------|
| E-Val-1 | 50 ms delay → honesty gate [45, 55] ms | ✅ PASS (5/5 runs) |

## E-Perf — Performance experiments (RQ1)

| ID | Purpose | Status |
|----|---------|--------|
| E-Perf-1 | Throughput: WAFER vs native vs eKuiper | ✅ shakedown clean |
| E-Perf-2 | Latency CDF comparison | ✅ shakedown clean |
| E-Perf-3 | Per-hop overhead with MQTT bookends | ✅ shakedown clean |
| E-Perf-4 | Per-hop overhead × payload size | ✅ shakedown clean |
| E-Perf-5 | Cross-architecture (x86 vs ARM) | ⚪ blocked — needs Pi |
| E-Perf-6 | Per-node RSS scaling | ✅ shakedown clean |
| E-Perf-7 | Metering overhead decomposition | ✅ shakedown clean |
| E-Perf-8 | Pipeline depth scaling | ✅ shakedown clean |
| E-Perf-9 | AOT cold vs warm startup | ✅ shakedown clean |

## E-Iso — Isolation experiments (RQ2)

| ID | Purpose | Status |
|----|---------|--------|
| E-Iso-1 | Buffer-overflow containment | ✅ contained |
| E-Iso-2 | Cross-node memory read | ✅ contained |
| E-Iso-3 | Unauthorized FS access | ✅ contained |
| E-Iso-4 | Infinite loop → epoch interrupt | ✅ contained |
| E-Iso-5 | Memory exhaustion → StoreLimits | ✅ contained |
| E-Iso-6 | Panic → pipeline continues | ✅ contained |
| E-Iso-7 | Parallel-branch fault isolation | ✅ <1% throughput impact |
| E-Iso-8 | Recovery time (trap → resume) | ✅ sub-ms |

## E-Swap — Hot-swap experiments (RQ3)

| ID | Purpose | Status |
|----|---------|--------|
| E-Swap-1 | Pause duration | ✅ p95 = 1.33 ms |
| E-Swap-2 | Zero-loss zero-duplication | ✅ 0 gaps, 0 dups |
| E-Swap-3 | Throughput dip vs full-restart | ✅ WAFER = 0 loss |
| E-Swap-4 | Swap under 2× burst | ✅ no degradation |
| E-Swap-5 | Failed swap recovery | 🟡 contained but no rollback (A17) |
| E-Swap-6 | Phase decomposition | ✅ 5 phases captured |

## E-Backpressure / E-Density

| ID | Purpose | Status |
|----|---------|--------|
| E-Backpressure | Burst → bounded queues | ✅ zero overflow |
| E-Density-1 | Binary size comparison | ✅ 12 plugins measured |

## Automation & reproducibility

| Item | Status |
|------|--------|
| `eval/` directory with configs + scripts | ✅ Complete |
| Python analysis notebooks | ✅ 11 canonical, statistical pipeline |
| Zenodo raw-data dataset | Not yet published |
| Pi setup automation (Ansible/script) | Not yet built |
| Hardware controls doc | Documented in canonical-readiness |

## Cross-references

- Canonical-readiness matrix: [`docs/status/canonical-readiness.md`](./canonical-readiness.md)
- Methodology-of-record: `tcc-doc/research/analysis/evaluation-plan.md`
- Harness design: `docs/rfcs/RFC-008-evaluation-harness.md`
- Result contract: `eval/RESULT-CONTRACT.md`
- Implementation gaps: `docs/status/implementation-gaps.md`
