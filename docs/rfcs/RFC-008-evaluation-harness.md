# RFC-008: Evaluation Harness Design

- **Status:** Implemented — production-path harness. RQ1/RQ3 benchmarks measure the real Wasm path (A15 closed 2026-07-20; A16 closed 2026-07-22; A17 closed 2026-08-02 with post-verify polish; A18 closed 2026-08-01; A19 closed 2026-08-02). Only residual gap at time of writing: **A20** (Prometheus `wafer_hot_swap_rollbacks_total` counter, observability follow-up, not blocking thesis) — see [`docs/status/implementation-gaps.md`](../status/implementation-gaps.md).
- **Original session date:** 2026-07-12
- **Depends on:** RFC-001 through RFC-007 (all prior architecture decisions)

> **Historical caveat (resolved).** Earlier revisions of this RFC warned
> that `crates/wafer-core/benches/throughput.rs` and `hot_swap.rs`
> measured the stub `TransformInstance` path (gap A15). That stub was
> removed on 2026-07-20; both benches now exercise
> `PluginTestHarness::load_transform` and
> `prepare_transform_swap_timed` against the production pass-through
> component. The `assert_no_stub_backed_evidence` guard in the source
> tree fails CI if the stub path returns.

## Abstract

This RFC designs the measurement infrastructure for WAFER's thesis evaluation: 22+ experiments across three research questions (performance overhead, fault containment, hot-swap disruption). The harness uses the real runtime as the system under test — special `BenchSource`/`BenchSink` adapters provide open-loop load generation with HdrHistogram recording, coordinated-omission-resistant timing (intended-publish-time stamps), and sequence tracking for loss/duplication detection. A native Rust baseline shares the same orchestrator and channels (eliminating confounders). Python UV-managed Jupyter notebooks handle statistical analysis (Mann-Whitney U, Bootstrap CI, Cliff's Delta). The design achieves <0.1% observer effect, full reproducibility via pinned environments and automated scripts, and a direct mapping from each experiment to the thesis figure it produces.

## Context

With the complete runtime architecture designed and optimized (Sessions 1–7), this session designs the evaluation harness — the measurement infrastructure that connects the runtime to the thesis evaluation plan. The harness must satisfy 22 experiments across 3 RQs with statistical rigor: N≥30, open-loop load generation, HdrHistogram recording, and warmup exclusion. Reproducibility is mandatory — any figure must be regenerable from published scripts and configs. The observer effect must remain below 0.1%.

Key constraints from prior sessions include: 4 metering configurations (Session 7 D6), AOT cache availability for cold/warm measurements (Session 7 D1), StoreLimits per node type (Session 7 D9), unconditional `NodeMetrics` with AtomicU64 counters (Session 5 D10), the 3-layer baseline stack (Session 5 D13), and the TestPipeline E2E builder (Session 6 D6).

The measurement infrastructure IS the pipeline — just with special source/sink adapters. No mocks, no separate benchmark binary, no observer effect >0.1%.

## Decisions

### Decision 1: Per-Hop Overhead Measurement Boundary

The per-hop timer starts AFTER `recv()` returns the envelope and ends AFTER the Wasm call completes (before downstream send). Includes fuel/epoch reset, ResourceTable operations, WIT canonical ABI marshaling, and the Wasm call itself. Excludes channel recv and channel send — these are identical in both Wasm and native baselines and cancel out.

Overhead: `Instant::now()` costs ~20–30ns. At 50µs per-hop target = 0.04–0.06%. AtomicU64 add = ~5ns. Total observer effect: <0.1%.

### Decision 2: Measurement Instrumentation — Unconditional Inline, Two Modes

A single `Instant::now()` pair is always present in the loop (unconditional `NodeMetrics`). For evaluation-grade recording, an optional `HdrHistogram` per node is attached when running in benchmark mode — enabled by the presence of `BenchSource`/`BenchSink` in the pipeline, not by conditional compilation or feature flags.

Two recording layers:
1. **Always-on** (production): `AtomicU64` counters — ~5ns overhead per message.
2. **Evaluation mode** (benchmark runs): Per-node `HdrHistogram` recording — ~20ns additional per message.

No separate benchmark binary. The same binary runs production and benchmarks — only the source/sink types differ.

### Decision 3: Open-Loop Load Generator — Dual Approach

Two complementary load generation mechanisms:

**A) In-Process `BenchSource`** (micro-benchmarks): A special source node that emits at constant arrival rate using `tokio::time::interval`. Stamps each message with `intended_publish_ns` (not actual send time) to prevent coordinated omission.

**B) External `wafer-loadgen` Binary** (E2E with MQTT): A separate Rust binary publishing to MQTT at constant arrival rate. Embeds timestamp in message payload. Uses token-bucket rate control.

Load profiles supported: steady (500/1000/2000 msg/s), burst (2× for 10s every 60s), ramp (100→5000 over 5 min), hot-swap trigger (steady + swap at t=120s).

### Decision 4: HdrHistogram Integration — Sink-Side Recording

One HdrHistogram instance per experiment run, recorded at the pipeline's terminal point (`BenchSink`). Per-node timing goes into unconditional AtomicU64 (aggregated post-hoc). Range: 1µs to 10s, 3 significant digits. A `SequenceTracker` detects gaps (lost messages) and duplicates — directly validates E-Swap-2 (zero loss, zero duplication).

### Decision 5: Native Rust Baseline — Same Crate, Trait-Based Swap

The native baseline lives in the same `wafer-core` crate, sharing the same orchestrator, channels, and envelope types. A `ProcessNode` trait allows swapping between Wasm and native implementations.

Three-layer baseline stack:
- Layer 0: single-flow — no channels, inline calls (direct function composition).
- Layer 1: native-with-channels — same Tokio tasks + mpsc + envelope.
- Layer 2: WAFER — full Wasm + WIT + isolation.

This is the Carbone 2015 gold standard: "reimplemented on the SAME RUNTIME."

### Decision 6: eKuiper Comparison Setup

Use WAFER's `wafer-loadgen` to drive MQTT messages to both systems identically. Same native Mosquitto broker, same message format, same Raspberry Pi 5 4 GB host, and same load profile. eKuiper 2.1.0 runs from its official Linux ARM64 package rather than in Docker, so neither comparator receives an additional container or bridge-network boundary.

The Pi boots with CPUs 1–3 isolated. The operating system, Mosquitto, and `wafer-loadgen` use CPU 0; the active SUT—WAFER, the native Rust baseline, or eKuiper—uses CPUs 1–3. Only one SUT runs at a time.

Measurement: both systems publish to an output topic. A common MQTT subscriber (part of wafer-loadgen) computes `now - payload.ts` for both — identical measurement methodology.

### Decision 7: Hot-Swap Measurement

Three complementary mechanisms:
- **A) Version markers** for E-Swap-1 (pause duration): plugins set a `plugin_version` metadata field during `init()`; `BenchSink` detects version boundary transitions.
- **B) Time-series throughput** for E-Swap-3 (throughput dip): periodic throughput sampling (1ms resolution) correlated with `NodeMetrics.swap_count` atomic increment.
- **C) Instrumented orchestrator** for E-Swap-6 (phase decomposition): `SwapTimeline` records `request_time`, `compile_done`, `instantiate_done`, `signal_sent`, `swap_acked`, `first_v2_output`.

### Decision 8: Attack Scenario Measurement

E-Iso-1 to E-Iso-6 are automated correctness tests using `TestPipeline` (pass/fail assertions). E-Iso-7 uses a parallel-branch topology where the fault node and measured node are on separate DAG branches to prove task-per-node isolation. E-Iso-8 measures recovery time from trap to first successful message after re-instantiation from `InstancePre`.

### Decision 9: Statistical Analysis — UV-Managed Python Notebooks

Rust handles recording (HdrHistogram + CSV). Python via UV-managed Jupyter notebooks handles analysis and figure generation. Statistical method per notebook: normality test (Shapiro-Wilk), Mann-Whitney U (non-normal) or t-test (normal), effect size via Cliff's Delta, Bootstrap 95% CI (10,000 resamples). Report: median, IQR, p95, p99.

Recording output per experiment run: `config.toml`, `metadata.json`, `latency.hdr`, `throughput.csv`, `per_node_metrics.csv`, `memory.csv`, `swap_timeline.json`, `sequence.csv`.

### Decision 10: Reproducibility Artifacts

Full automation suite in `eval/` directory: pipeline configs (telemetry, inference, passthrough variants ×4 metering, native, depth-1/2/3/5/10), load generator profiles, Raspberry Pi 5 host setup and preflight scripts, experiment runner (`run-experiment.sh`), and mise tasks for targeting individual experiments.

Published alongside thesis: git repository, raw results tarball (Zenodo), exact binary SHA256 for all `.wasm` modules, `Cargo.lock` pinning wasmtime version, hardware/OS manifest, `uv.lock` for Python environment.

### Decision 11: Warmup Detection — 30s Exclusion + ADF Verification

30s warmup exclusion (messages discarded by `BenchSink`) with post-hoc Augmented Dickey-Fuller stationarity verification in notebook `00-warmup-validation.ipynb`. At 1000 msg/s = 30,000 discarded messages, more than sufficient for Tokio stabilization and cache warming. ADF provides statistical proof that remaining data is stationary.

### Decision 12: Throughput Saturation — Ramp + Latency Threshold

"Saturated" = the rate where p99 latency exceeds 2× the p99 at steady-state (1000 msg/s) OR message loss exceeds 1%. Procedure: establish baseline at 1000 msg/s, run ramp profile (100→5000), binary search refinement between last-good and first-bad rates (30s steady runs). Follows Karimov 2018's "sustainable throughput" definition.

### Decision 13: Cross-Architecture — Same Source Revision, Ratio Reporting

Build equivalent release binaries from the same tagged source revision for ARM64 (Raspberry Pi 5) and x86-64. Report overhead RATIO (Wasm/Native) — dimensionless, portable across hardware. Target: |ARM ratio – x86 ratio| < 5 percentage points.

### Decision 14: Memory Measurement — `/proc/self/statm` at 1Hz

Use the runtime's `memory-stats` integration to record process RSS at 1 Hz. For per-node attribution (E-Perf-6), measure delta RSS when adding nodes incrementally (1→3→5→10 nodes); per-node cost = slope of linear fit. Canonical memory numbers come from the Raspberry Pi 5 Linux host.

### Decision 15: Shared Pipeline Builder — Pluggable I/O Adapters

One unified `PipelineBuilder` with pluggable source/sink types. `TestPipeline` and `BenchmarkPipeline` are configuration presets, not separate infrastructure. Source types: `MemorySource`, `BenchSource`, `MqttSource`. Sink types: `CollectorSink`, `BenchSink`, `NullSink`. The real orchestrator IS the system under test.

## Alternatives Considered

- **Separate benchmark binary**: Rejected because a different binary would have different Tokio configuration, allocation patterns, and compilation flags — all confounders. Using the same binary with special source/sink adapters eliminates code divergence.
- **Conditional compilation (`#[cfg(feature = "bench")]`)**: Rejected because it creates code divergence between "what we measure" and "what we ship." The observer effect is provably <0.1% even with both measurement layers active.
- **Rust-only analysis (stats-claw / anofox-statistics crates)**: Rejected in favour of Python notebooks — thesis figures benefit from matplotlib/seaborn maturity, and the UV lockfile provides reproducible environments. Rust handles recording; Python handles analysis.
- **External measurement tools (perf, strace, eBPF)**: Rejected for primary metrics because they add non-trivial overhead and are harder to reproduce across hardware. Used only for validation/cross-checking, not primary data.
- **Fixed-bucket histograms**: Rejected in favour of HdrHistogram which provides constant-time recording (~20ns), fixed memory footprint, and lossless percentile accuracy across the full latency range.
- **Event-time-based measurement (embedding timestamps in MQTT payload only)**: The external `wafer-loadgen` uses payload timestamps, but in-process micro-benchmarks use `Instant::now()` for nanosecond precision without serialization overhead. Both approaches coexist for their respective experiment classes.

## Canonical hardware amendment

Raspberry Pi 5 with 4 GB RAM is the canonical gateway target. Native eKuiper 2.1.0 replaces the Docker comparator on that host. CPU 0 runs operating-system work, native Mosquitto, and `wafer-loadgen`; CPUs 1–3 run exactly one active SUT. The rationale, comparability limitation, and migration scope are frozen in [`docs/status/rpi5-canonical-transition.md`](../status/rpi5-canonical-transition.md) before Pi 5 measurements begin.

## Related RFCs

- **RFC-005** (Orchestrator) — provides the unconditional `NodeMetrics` (AtomicU64 counters, D10) and task-per-node structure (D1) that the harness instruments.
- **RFC-006** (Plugin SDK) — provides the `TestPipeline` builder concept (D6) generalized here into the shared `PipelineBuilder` with pluggable I/O adapters.
- **RFC-007** (Performance Optimizations) — provides the 4 metering configurations (D6), AOT cache (D1), StoreLimits (D9), and epoch ticker (C1) that the harness decomposes in E-Perf-7.
- **RFC-001** (WIT Contracts) — defines the typed boundary being measured in per-hop overhead experiments.
- **RFC-003** (Node Types) — defines the five node categories whose overhead is measured individually.

## Implementation Notes

- The production-path runtime, load generator, result metadata, and analysis notebooks are implemented. The Pi 5 deployment, preflight, native-eKuiper adapter, and canonical experiment wrappers are tracked by the active `rpi5-canonical-runs` plan.
- `eval/configs/` contains telemetry, passthrough, metering-ablation, native-baseline, depth-scaling, isolation, and hot-swap configurations. A runbook identifies which configurations already have executable runners and which canonical wrappers remain to be completed.
- The `wafer-loadgen` crate exists (`crates/wafer-loadgen/src/main.rs`) as a workspace binary.

### Amendments to prior RFCs noted in source

- Session 5 (RFC-005) D3 node loop gains a `ProcessNode` trait abstraction (Decision 5 here).
- Session 6 (RFC-006) D6 `TestPipeline` is generalized to a shared `PipelineBuilder` (Decision 15 here).
- Session 7 (RFC-007) D8 "benchmark-first strategy" is fully specified by this RFC's Implementation Priority section (~28 hours total).
