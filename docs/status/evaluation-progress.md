# Evaluation progress

## Current final-campaign readiness

The final Raspberry Pi 5 method is implemented through canonical analysis. The final N=30 campaign has not started.

| Area | Status |
|---|---|
| Final matrix and result contract | frozen and validated |
| Canonical WAFER metering | explicit fuel plus epoch, with declared exceptions |
| E-Perf-10 bounded capacity capture | implemented |
| E-Swap-3 actual-t0 event buckets | implemented |
| E-Swap-4 source-driven burst | implemented |
| Canonical analysis and approval gate | implemented |
| WAFER/thesis documentation sync | in progress |
| Tagged release candidate | pending |
| Reduced targeted Pi pilot | pending |
| Independent final-readiness review | pending |
| Human approval | pending |
| Full N=30 campaign | not started |

The matrix contains 2,105 schedule records and 1,893 executed or static leaves. The final capacity grid is `[1,000, 4,000, 8,000, 15,000, 16,000]` msg/s for MQTT loopback, Native, protected WAFER, and eKuiper.

## Current experiment boundaries

- E-Perf-1 is the 1,000 msg/s target-load comparison.
- E-Perf-10 is the common-grid gateway-capacity envelope with MQTT support censoring.
- E-Perf-5 remains `PENDING` until matching x86 Linux evidence exists.
- E-Perf-9 measures Linux filesystem page-cache state with the disk compiled-component cache disabled.
- E-Swap-3 measures one event-aligned disruption in each of 30 runs per strategy.
- E-Swap-4 measures one true 1,000/2,000/1,000 msg/s burst and one stateless swap in each of 30 runs.
- PMIC telemetry is an internal-rail proxy, not total board power.

See [canonical readiness](canonical-readiness.md), [RFC-008](../rfcs/RFC-008-evaluation-harness.md), and [the Pi 5 runbook](../eval/pi5-experiment-runbook.md).

## Historical diagnostic program

<!-- historical-diagnostic-below -->

The macOS shakedown program and Raspberry Pi pilot/scout batches validated implementation paths and exposed method defects. Those results remain in immutable result trees and historical reports. They are not pooled with the final batch and do not supply final RQ verdicts.

Closed implementation findings include the WASI async path, process-time rollback, native filter parity, runtime-owned memory sampling, explicit eKuiper QoS, true metering ablation, trace-free capacity capture, event-aligned restart evidence, and true-burst scheduling.

The accepted capacity scout is diagnostic only. It selected the common rate grid before final execution and established that higher SUT capacity may be censored by the co-located MQTT support path.

## Preserved original record

<!-- historical-diagnostic-below -->

The following text is the earlier decision or diagnostic record. It is preserved for traceability and does not override the current sections above.

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

## What's next: canonical runs on Raspberry Pi 5

The canonical-readiness matrix (`docs/status/canonical-readiness.md`)
documents every experiment's gap list, macOS confounders, and what must be
verified on the Raspberry Pi 5 4 GB host. The immediate path is:

1. ✅ Deploy ARM64 binaries and evaluation plugins to the Pi 5.
2. ✅ Boot with `isolcpus=1-3`; assign CPU 0 to OS/Mosquitto/loadgen and CPUs 1–3 to the active SUT.
3. ✅ Pass Pi 5 preflight, Pipeline C smoke, and the E-Val-1 honesty check. The 2026-08-29 dirty-tree shakedown is methodology evidence only.
4. ✅ Implement one reviewed, resumable canonical runner with N≥30, experiment-specific warmup, strict provenance, and PMIC telemetry.
5. Run the frozen matrix against WAFER, native Rust, and native eKuiper 2.1.0 on the Pi 5.

## Runtime infrastructure

| Component | Status |
|-----------|--------|
| Open-loop load generator (`wafer-loadgen`) | ✅ Fully operational |
| Native Rust baseline (Pipeline D) | ✅ Passthrough functional |
| eKuiper comparison setup | ✅ Native ARM64 install, seed, and smoke path; canonical wrappers pending |
| Attack scenario Wasm modules (S1–S6) | ✅ All 6 implemented and tested |
| Per-hop latency instrumentation | ✅ HdrHistogram + sequence tracker |
| Per-node RSS sampling | ✅ Runtime-owned 1 Hz `memory-stats` sampler |
| Pipeline configs (1/3/5/10-node chains) | ✅ All checked in |
| Analysis notebooks | ✅ 11 canonical + 4 auxiliary |

## E-Val — Methodology validation

| ID | Purpose | Status |
|----|---------|--------|
| E-Val-1 | 50 ms delay → honesty gate [45, 55] ms | ✅ macOS 5/5; Pi 5 shakedown p99 51.184 ms, 300/300 messages, zero gaps |

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
| Pi setup automation | ✅ Pi 5 setup, deploy, preflight, smoke, and validation scripts |
| Hardware controls doc | ✅ `docs/eval/pi5-host-setup.md` |
| Pi 5 hardware shakedown | ✅ 19/19 preflight; Pipeline C and E-Val-1 passed; dirty-tree/non-canonical |

## Cross-references

- Canonical-readiness matrix: [`docs/status/canonical-readiness.md`](./canonical-readiness.md)
- Methodology-of-record: `tcc-doc/research/analysis/evaluation-plan.md`
- Harness design: `docs/rfcs/RFC-008-evaluation-harness.md`
- Result contract: `eval/RESULT-CONTRACT.md`
- Implementation gaps: `docs/status/implementation-gaps.md`
