# RFC-008: Evaluation harness design

- **Status:** Implemented for final-campaign readiness
- **Original session date:** 2026-07-12
- **Final-method amendment:** 2026-09-04
- **Depends on:** RFC-001 through RFC-007

## Abstract

The evaluation harness measures the production WAFER runtime on a Raspberry Pi 5 4 GB gateway. It uses the real Wasm Component Model path, bounded open-loop generators, HdrHistogram latency recording, sequence accounting, process and thermal telemetry, and a native eKuiper 2.1.0 comparator. The final schedule and experiment parameters come from `eval/canonical-matrix.json`; artifact schemas come from `eval/RESULT-CONTRACT.md`.

The independent unit is a complete process run unless the matrix explicitly declares a static or repeated-event experiment. Evidence classes are `canonical-primary`, `candidate-supplementary`, `diagnostic`, and `future-work`. Diagnostic scout, shakedown, focused-pilot, targeted-pilot, and candidate-supplementary batches remain separate from canonical-primary evidence. A candidate is not automatically admitted to N=30; a post-rehearsal selection receipt must record `include`, `defer`, or `reject` first.

## Measurement boundary

The matched gateway boundary is the complete co-located host:

- CPU 0 runs Linux support work, native Mosquitto, load generation, subscription, and telemetry.
- CPUs 1-3 run one active SUT: Native, protected WAFER, or native eKuiper.
- The MQTT loopback condition measures the shared support path.
- QoS 1, payload, topics, publisher, subscriber, warmup, duration, and CPU allocation are matched.

Pipeline A is:

```text
MQTT source -> threshold filter -> MQTT sink
```

WAFER uses the Wasm threshold-filter component, Native uses the equivalent Rust filter, and eKuiper uses one SQL filter operator with concurrency 1.

## Recording paths

### In-process path

`BenchSource` and `BenchSink` support boundary, depth, memory, containment, and hot-swap experiments. `BenchSource` uses open-loop intended timestamps. `BenchSink` records latency, throughput, sequence continuity, and version transitions without replacing the production orchestrator.

E-Swap-4 adds one deterministic source schedule after a 30-second warmup:

```text
measured [0 s, 55 s):   1,000 msg/s
measured [55 s, 65 s):  2,000 msg/s
measured [65 s, 120 s): 1,000 msg/s
swap at measured 60 s
```

Each of 30 independent runs contains one stateless swap and contributes one sink-observed gap to the across-run p95.

### External MQTT path

`wafer-loadgen publish` and `wafer-loadgen subscribe` drive E-Perf-1, E-Perf-2, E-Perf-10, and E-Swap-3 through the same native Mosquitto broker. The publisher distinguishes intended offers, client-queue rejection, and successful enqueue. The subscriber records bounded sequence and HDR summaries. Final capacity runs do not require per-message CSV traces.

## Canonical metering

Runtime configuration defaults are unmetered: omitted fuel budgets and an omitted epoch deadline deserialize to `None`. Final WAFER evaluation configs explicitly protect ordinary leaves with:

| Mechanism | Final evaluation value |
|---|---:|
| Transform fuel | 10,000,000 per call |
| Filter fuel | 500,000 per call |
| Router fuel | 500,000 per call |
| Epoch deadline | 100 ticks |
| Epoch tick | 10 ms |

Only matrix-declared E-Perf-7 ablations and attack-specific stimuli differ. E-Perf-7 uses omission to create the four parsed modes: neither `(None, None)`, fuel-only `(Some, None)`, epoch-only `(None, Some)`, and both `(Some, Some)`. Numeric sentinel values do not disable metering.

## Final RQ1 experiments

### Matched target load

E-Perf-1 compares Native, protected WAFER, and eKuiper at 1,000 msg/s for 30 complete runs per condition. It reports delivery, loss, run-level p50/p95/p99, CPU, RSS, thermal state, and provenance. It is an operating-point comparison, not a capacity experiment.

### Gateway-capacity envelope

E-Perf-10 runs MQTT loopback, Native, protected WAFER, and eKuiper at the common grid:

```text
[1,000, 4,000, 8,000, 15,000, 16,000] msg/s
```

Every system-rate condition has 30 independent runs. A rate is delivery-good when pooled loss is at most 1 percent, the mean achieved/offered ratio is at least 0.99, and duplicate count is zero. Analysis reports the highest tested delivery-good rate and the first support-uncensored rate whose median normalized run p99 exceeds 2.0.

A delivery-bad MQTT loopback rate support-confounds SUT results at that rate and above. The analysis must report censoring rather than assign an exact SUT ceiling beyond the shared support path.

### Startup and cross-architecture boundaries

E-Perf-9 compares Linux filesystem page-cache preparation. The runtime disk compiled-component cache is disabled for these runs, so E-Perf-9 is not evidence for AOT or serialized-component caching.

E-Perf-5 remains `PENDING` until matching Raspberry Pi 5 and x86 Linux batches exist at the same source and method. The x86 execution and any cross-architecture conclusion remain `future-work`.

## Final RQ3 experiments

### E-Swap-3 event-aligned disruption

Each strategy has 30 runs and one disruption at measured t=60 seconds:

- WAFER stateless hot-swap;
- WAFER process restart;
- eKuiper rule restart.

The subscriber retains bounded observations and writes exactly 200 contiguous 100 ms buckets over `[-10 s,+10 s)` around the actual action-start timestamp. The scheduled t=60 boundary and actual alignment error are recorded; an error over 10 ms invalidates the leaf.

The run-level estimators are:

- baseline median rate over `[-10,-2)`;
- minimum event-window rate over `[-2,+2)`;
- percentage dip from baseline;
- contiguous below-95-percent interruption containing t0;
- action duration from a monotonic clock;
- recovery to five consecutive buckets at or above 95 percent of baseline after action end, right-censored at +10 seconds;
- full-run loss, duplication, and latency summary.

The restart comparators are measured rather than assigned a synthetic 100 percent dip.

### E-Swap-4 true burst

E-Swap-4 uses 30 independent runs of the source schedule shown above. Each run records source and sink phase populations, 1,200 source-origin 100 ms primary buckets over `[0,120s)`, a separate 100-bucket drain series over `[120s,130s)`, the scheduled and actual swap boundary, sequence integrity, internal phases, and one sink-observed gap. The source carries `bench.measurement_start_unix_ns`; sink offsets use the explicitly labeled `unix-epoch-source-sink-alignment` clock while scheduling and source-completion duration remain monotonic. Full-run counts reconcile the primary, drain, and O(1) after-drain counters. Any receive at or after 130 seconds, right-censored drain, loss, or duplication rejects the run. Reconciled drain arrivals remain separate completion evidence and do not enter the t=60 disruption estimator. The previous constant-2,000 msg/s repeated-swap pilot is diagnostic only.

## Statistics and outputs

Canonical analysis uses complete runs as independent units, run-level bootstrap 95 percent confidence intervals, and non-parametric effect sizes where applicable. Intervals and repeated swap events are nested observations, not independent replicates. Candidate-supplementary runs are not pooled with canonical-primary or prior rehearsal runs. The notebooks fail closed on unapproved, incomplete, dirty, mixed-SHA, throttled, failed, or malformed canonical input. Explicit diagnostic paths remain descriptive and render missing inputs as `PENDING`.

Figures and tables state N, units, estimator, evidence class, and claim boundary. Percentile summaries are not presented as empirical CDFs. PMIC measurements are labeled as a Raspberry Pi 5 internal-rail proxy, not total board, USB-C input, or total input power. External input-power capture remains `future-work`.

E-Perf-2 remains an alternate analysis of E-Perf-1, E-Perf-8 of E-Perf-6, and E-Swap-2/E-Swap-6 of E-Swap-1. These aliases do not multiply sample counts. E-Swap-4 retains 1,200 source-origin primary buckets over `[0,120s)`, 100 separate drain buckets over `[120s,130s)`, zero after-drain arrivals, and no accepted right censoring.

## Enhanced evidence storage

V5 raw evidence lives as one physical copy on the exFAT volume labeled `WAFER_RESULTS`. Pi and Jetson use `/mnt/wafer-results`; macOS uses `/Volumes/WAFER_RESULTS`. Manifests record volume-root-relative paths. The same full SHA-256 manifest is verified after each mount or host transition, and the drive is synchronized and unmounted cleanly before physical movement.

Raw attempts are append-only, including failed and interrupted attempts. Analysis opens `raw/` read-only and writes only to `derived/` and `reports/`. The method does not depend on symlinks, hardlinks, case-only path distinctions, or POSIX ownership persistence, and it never creates a second raw-data copy.

The retained 5 V / 4.2 A supply is admitted empirically. It receives no threshold waiver for nonzero throttling, high temperature, reboot, kernel I/O errors, or checksum failure.

## Reproducibility

A final leaf contains clean tagged provenance, config and binary identities, thermal/throttle state, experiment-specific artifacts, and a passed completion receipt. The canonical runner is sequential and resumable. It writes a deterministic schedule and never overwrites a passed attempt.

Use these gates before interpreting a batch:

```sh
python3 eval/scripts/validate-canonical.py matrix eval/canonical-matrix.json
python3 -m pytest -q eval/scripts/tests
python3 eval/scripts/verify-result-contract.py --canonical <result-directory>
```

Canonical notebook resolution also requires the human approval receipt described by `eval/analysis/notebooks/README.md`.

## Related documents

- [Result contract](../../eval/RESULT-CONTRACT.md)
- [Pi 5 experiment runbook](../eval/pi5-experiment-runbook.md)
- [Config schema](../interfaces/config-schema.md)
- [ADR-0013](../adr/0013-aot-cache-and-metering.md)
- [Canonical readiness](../status/canonical-readiness.md)

## Preserved original record

<!-- historical-diagnostic-below -->

The following text is the earlier decision or diagnostic record. It is preserved for traceability and does not override the current sections above.

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

Key constraints from prior sessions include: 4 metering configurations (Session 7 D6), a dormant compiled-component cache design (Session 7 D1), StoreLimits per node type (Session 7 D9), unconditional `NodeMetrics` with AtomicU64 counters (Session 5 D10), the 3-layer baseline stack (Session 5 D13), and the TestPipeline E2E builder (Session 6 D6).

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

E-Iso-1 to E-Iso-6 are automated correctness tests using `TestPipeline` (pass/fail assertions). E-Iso-7 uses independent source and sink populations for the healthy and fault branches, and reports branch-A throughput and latency under matched panic and epoch-loop attacks. A shared source is invalid for this experiment because lossless fan-out propagates a fault branch's backpressure into the healthy branch before either sink. `BenchSource` marks the exact warmup population, and `BenchSink` applies that marker to sequence, latency, and throughput accounting. All E-Iso-7 conditions use the production-aligned 100-tick, 10 ms epoch budget: shorter diagnostic budgets can interrupt healthy calls during ordinary scheduler delays and confound isolation with timeout sensitivity. E-Iso-8 measures recovery time from trap to first successful message after re-instantiation from `InstancePre`.

### Decision 9: Statistical Analysis — UV-Managed Python Notebooks

Rust handles recording (HdrHistogram + CSV). Python via UV-managed Jupyter notebooks handles analysis and figure generation. Statistical method per notebook: normality test (Shapiro-Wilk), Mann-Whitney U (non-normal) or t-test (normal), effect size via Cliff's Delta, Bootstrap 95% CI (10,000 resamples). Report: median, IQR, p95, p99.

Recording output per experiment run: `config.toml`, `metadata.json`, `latency.hdr`, `throughput.csv`, `per_node_metrics.csv`, `memory.csv`, `swap_timeline.json`, `sequence.csv`.

### Decision 10: Reproducibility Artifacts

Full automation suite in `eval/` directory: pipeline configs (telemetry, inference, passthrough variants ×4 metering, native, depth-1/2/3/5/10), load generator profiles, Raspberry Pi 5 host setup and preflight scripts, experiment runner (`run-experiment.sh`), and mise tasks for targeting individual experiments.

Published alongside thesis: git repository, raw results tarball (Zenodo), exact binary SHA256 for all `.wasm` modules, `Cargo.lock` pinning wasmtime version, hardware/OS manifest, `uv.lock` for Python environment.

### Decision 11: Warmup Detection — 30s Exclusion + ADF Verification

30s warmup exclusion (messages discarded by `BenchSink`) with post-hoc Augmented Dickey-Fuller stationarity verification in notebook `00-warmup-validation.ipynb`. At 1000 msg/s = 30,000 discarded messages, more than sufficient for Tokio stabilization and cache warming. ADF provides statistical proof that remaining data is stationary.

### Decision 12: Throughput Saturation — Frozen Fixed-Rate Sweep

"Saturated" = the first frozen rate where the median run p99 exceeds 2× the median p99 at 1000 msg/s or aggregate loss exceeds 1%. E-Perf-10 uses open-loop steady points at 500, 1000, 2000, 4000, 8000, and 16000 msg/s for MQTT loopback, native Rust, WAFER, and eKuiper. The publisher does not wait for capacity in its bounded MQTT client queue; rejected offers remain visible as sequence loss. Warmup uses a disjoint sequence range, and the subscriber excludes delayed warmup output from the measured population while recording the excluded count. Four diagnostic repetitions use seeded rate blocks with rotated system order. Sustainable throughput is the last contiguous good point at or above the baseline; if no point fails, the result states that saturation was not observed within the tested range. Rates are not refined after results are observed. This follows Karimov 2018's sustainable-throughput definition while preventing comparator-specific adaptive sampling.

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
- **RFC-007** (Performance Optimizations) — defines the 4 metering configurations (D6), a compiled-cache design that is not wired into runtime startup (D1), StoreLimits (D9), and the epoch ticker (C1).
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
