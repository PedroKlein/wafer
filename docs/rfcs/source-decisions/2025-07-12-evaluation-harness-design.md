# Evaluation Harness Design — Session 8 Decisions

**Date:** 2025-07-12  
**Status:** Decided (pending implementation)  
**Scope:** Phase 0 structural refactor — measurement infrastructure, load generation, recording, analysis  
**Depends on:** Sessions 1–7 (all prior architecture decisions)  
**Feeds into:** Implementation task graph, thesis evaluation chapter, tcc-doc evaluation-plan.md  

---

## TL;DR — How Measurement Works

```
1. BenchSource emits messages at constant rate with intended-publish-time stamps
2. Messages flow through the REAL pipeline (same orchestrator as production)
3. Each node loop records per-hop timing into unconditional AtomicU64 counters
4. BenchSink records end-to-end latency into HdrHistogram, tracks sequence gaps
5. After run: export .hdr + .csv files → Python notebooks analyze + generate thesis figures
```

The measurement infrastructure IS the pipeline — just with special source/sink adapters. No mocks, no separate benchmark binary, no observer effect >0.1%.

---

## Context

With the complete runtime architecture designed and optimized (Sessions 1–7), this session designs the evaluation harness — the measurement infrastructure that connects the runtime to the thesis evaluation plan. The harness must satisfy:

- 22 experiments across 3 RQs
- Statistical rigor: N≥30, open-loop, HdrHistogram, warmup exclusion
- Reproducibility: any figure regenerable from published scripts + configs
- Minimal observer effect: <0.1% overhead from measurement instrumentation
- Support for 3 baselines: native Rust, eKuiper, full restart

**Key constraints from prior sessions:**
- 4 metering configurations (both/fuel-only/epoch-only/neither) — Session 7 D6
- AOT cache available → cold-start vs warm-start measurements — Session 7 D1
- StoreLimits per-type (Transform 64MB, Filter/Router 16MB) — Session 7 D9
- Unconditional `NodeMetrics` (AtomicU64 always-on) — Session 5 D10
- `swap_count` in NodeMetrics — Session 5 D10
- DLQ envelope with `original.id` — Session 5 D8
- 3-layer baseline stack — Session 5 D13
- Epoch ticker on OS thread — Session 7 C1
- TestPipeline E2E builder — Session 6 D6
- Task-per-node → per-node metrics isolation — Session 5 D1

---

## Decision 1: Per-Hop Overhead Measurement Boundary

**Decision:** The per-hop timer starts AFTER `recv()` returns the envelope and ends AFTER the Wasm call completes (before downstream send). Includes fuel/epoch reset, ResourceTable operations, WIT canonical ABI marshaling, and the Wasm call itself. Excludes channel recv and channel send.

**Rationale:** "Per-hop overhead" = everything the Wasm boundary adds that a native function call wouldn't. Channel operations are identical in both the Wasm and native baselines — they cancel out. Including fuel/epoch reset captures the FULL isolation cost, which is exactly what the 4-config decomposition (Session 7 D6) decomposes.

**Implementation:**
```rust
// In the node loop (transform example):
let envelope = receiver.recv().await; // NOT included

// ─── MEASUREMENT START ───
let hop_start = Instant::now();

if fuel_enabled { store.set_fuel(fuel_limit)?; }
if epoch_enabled { store.set_epoch_deadline(deadline); }
let result = transform.process(envelope).await;

let hop_ns = hop_start.elapsed().as_nanos() as u64;
// ─── MEASUREMENT END ───

metrics.total_process_ns.fetch_add(hop_ns, Relaxed);
metrics.messages_processed.fetch_add(1, Relaxed);
// Then: send downstream (NOT included)
```

**Overhead:** `Instant::now()` costs ~20-30ns. At 50µs per-hop target = 0.04-0.06%. AtomicU64 add = ~5ns. Total observer effect: <0.1%.

**Interactions:** Directly maps to E-Perf-4. The 4-config decomposition (E-Perf-7) reveals which portion of this measurement comes from fuel vs epoch vs pure Wasm boundary.

---

## Decision 2: Measurement Instrumentation — Unconditional Inline, Two Modes

**Decision:** A single `Instant::now()` pair is always present in the loop (already decided: unconditional `NodeMetrics`). For evaluation-grade recording, an optional `HdrHistogram` per node is attached when running in benchmark mode — enabled by the presence of `BenchSource`/`BenchSink` in the pipeline, not by conditional compilation or feature flags.

**Two recording layers:**
1. **Always-on** (production): `AtomicU64` counters — ~5ns overhead per message
2. **Evaluation mode** (benchmark runs): Per-node `HdrHistogram` recording — ~20ns additional per message

**Rationale:** No conditional compilation needed. The observer effect is provably <0.1% even with both layers active. This avoids code divergence between "what we measure" and "what we ship."

**No separate benchmark binary.** The same binary runs production and benchmarks — only the source/sink types differ.

---

## Decision 3: Open-Loop Load Generator — Dual Approach

**Decision:** Two complementary load generation mechanisms:

### A) In-Process `BenchSource` (micro-benchmarks)
A special source node that emits at constant arrival rate using `tokio::time::interval`. Stamps each message with `intended_publish_ns` (not actual send time) to prevent coordinated omission.

```rust
pub struct BenchSource {
    payloads: Vec<Vec<u8>>,
    rate_per_sec: f64,
    total_messages: u64,
    warmup_messages: u64,
    sequence: u64,
    start_time: Instant,
}

// Emission: constant arrival rate
let interval = Duration::from_secs_f64(1.0 / self.rate_per_sec);
let mut ticker = tokio::time::interval(interval);
for seq in 0..self.total_messages {
    ticker.tick().await;
    let intended_ns = (seq as f64 * interval.as_nanos() as f64) as u64;
    let envelope = RuntimeEnvelope::bench(seq, intended_ns, &self.payloads[seq % len]);
    sender.send(envelope).await?;
}
```

### B) External `wafer-loadgen` Binary (E2E with MQTT)
A separate Rust binary publishing to MQTT at constant arrival rate. Embeds timestamp in message payload. Uses token-bucket rate control (lightbench pattern).

```rust
// wafer-loadgen/src/main.rs
let mut rate = RateController::new(target_rate);
loop {
    rate.wait_for_next().await;
    let ts = now_unix_ns();
    let payload = format!(r#"{{"ts":{},"seq":{},"device_id":"bench","temperature":42.5}}"#, ts, seq);
    mqtt_client.publish(topic, QoS::AtMostOnce, false, payload).await?;
    seq += 1;
}
```

**Rationale:**
- In-process avoids MQTT broker overhead for micro-benchmarks (E-Perf-4, E-Perf-6, E-Perf-8)
- External separates load generator from SUT for E2E (Delmonte 2020 pattern)
- Intended-publish-time stamping prevents coordinated omission (Tene 2012)

**Load profiles supported:**

| Profile | Implementation |
|---------|---------------|
| Steady (500/1000/2000 msg/s) | Fixed `interval` |
| Burst (2× for 10s every 60s) | Dynamically adjust `interval` |
| Ramp (100→5000 over 5 min) | Linearly decrease `interval` |
| Hot-swap trigger (steady + swap at t=120s) | Steady + orchestrator API call at scheduled time |

---

## Decision 4: HdrHistogram Integration — Sink-Side Recording

**Decision:** One HdrHistogram instance per experiment run, recorded at the pipeline's terminal point (`BenchSink`). Per-node timing goes into unconditional AtomicU64 (aggregated post-hoc).

```rust
pub struct BenchSink {
    histogram: Histogram<u64>,           // 3 sig digits, 1µs–10s range
    warmup_until: Instant,
    sequence_tracker: SequenceTracker,   // gap/duplicate detection
    throughput_samples: Vec<(Instant, u64)>, // periodic throughput snapshots
    csv_writer: Option<CsvWriter>,
    message_count: u64,
}

impl BenchSink {
    pub fn new(config: BenchSinkConfig) -> Self {
        Self {
            // Range: 1µs (1000ns) to 10s (10_000_000_000ns), 3 significant digits
            histogram: Histogram::new_with_bounds(1_000, 10_000_000_000, 3).unwrap(),
            warmup_until: Instant::now() + Duration::from_secs(config.warmup_secs),
            sequence_tracker: SequenceTracker::new(),
            ..
        }
    }

    fn on_message(&mut self, envelope: &RuntimeEnvelope) {
        if Instant::now() < self.warmup_until { return; }
        
        let latency_ns = now_ns() - envelope.timestamp;
        self.histogram.record(latency_ns).ok();
        self.sequence_tracker.record(envelope.sequence);
        self.message_count += 1;
    }
}
```

**For per-hop measurement (E-Perf-4):** Separate `NodeLatencyRecorder` with per-node histogram, enabled during micro-benchmark runs only.

**SequenceTracker:** Detects gaps (lost messages) and duplicates — directly validates E-Swap-2 (zero loss, zero duplication). Inspired by lightbench's `SequenceTracker`.

**Rationale:** Sink-side records the full end-to-end latency (what users experience). HdrHistogram's constant-time recording (~20ns) and fixed memory footprint make it ideal for high-volume sustained benchmarks.

---

## Decision 5: Native Rust Baseline — Same Crate, Trait-Based Swap

**Decision:** The native baseline lives in the same `wafer-core` crate, sharing the same orchestrator, channels, and envelope types. A `ProcessNode` trait allows swapping between Wasm and native implementations.

```rust
pub trait ProcessNode: Send + 'static {
    async fn process(&mut self, envelope: RuntimeEnvelope) -> Result<RuntimeEnvelope, ProcessError>;
}

// Wasm implementation:
impl ProcessNode for WasmTransform { /* fuel reset, WIT marshal, Wasm call */ }

// Native implementation (baseline):
pub struct NativeTransform {
    process_fn: Box<dyn Fn(RuntimeEnvelope) -> Result<RuntimeEnvelope, ProcessError> + Send>,
}
impl ProcessNode for NativeTransform {
    async fn process(&mut self, envelope: RuntimeEnvelope) -> Result<RuntimeEnvelope, ProcessError> {
        (self.process_fn)(envelope)
    }
}
```

**Pipeline D config:**
```toml
[nodes.parse]
type = "native-transform"
function = "json_parse"

[nodes.filter]
type = "native-filter"
function = "threshold_filter"
config = '{"field": "temperature", "threshold": 50}'
```

**3-layer stack (Session 5 D13) implementation:**

| Layer | Config | What it uses |
|-------|--------|--------------|
| Layer 0: single-flow | No channels, inline calls | Direct function composition |
| Layer 1: native-with-channels | `type = "native-transform"` | Same Tokio tasks + mpsc + envelope |
| Layer 2: WAFER | `type = "transform"` | Full Wasm + WIT + isolation |

**Rationale:** Same orchestrator + same channels + same envelope eliminates all confounding variables except Wasm. This is the Carbone 2015 gold standard: "reimplemented on the SAME RUNTIME." A separate binary would have different Tokio configuration, allocation patterns, and compilation flags — all confounding.

---

## Decision 6: eKuiper Comparison Setup

**Decision:** Use WAFER's `wafer-loadgen` to drive MQTT messages to both systems identically. Same MQTT broker (mosquitto), same message format, same hardware, same load profile. Same-machine deployment with CPU affinity isolation.

**Architecture:**
```
wafer-loadgen (core 0) → Mosquitto (core 0) → WAFER (cores 1-3) → MQTT output
                                             → eKuiper (cores 1-3) → MQTT output
                                             ↑ (run separately, same config)
```

**eKuiper SQL equivalent:**
```sql
CREATE STREAM telemetry (device_id string, temperature float, humidity float)
  WITH (DATASOURCE="wafer/bench/input", FORMAT="JSON");

CREATE RULE threshold_alert AS
  SELECT * FROM telemetry WHERE temperature > 50
  INTO "wafer/bench/output";
```

**Measurement:** Both systems publish to an output topic. A common MQTT subscriber (part of wafer-loadgen) computes `now - payload.ts` for both — identical measurement methodology.

**Fairness guarantees:**
- Same MQTT broker, same message format, same payload (~120 bytes JSON)
- Same load profiles (500/1000/2000 msg/s)
- Same hardware (RPi 4), same CPU affinity configuration
- Same measurement methodology (subscriber-side latency)
- eKuiper given equivalent compute (no isolation overhead — that's the point of the comparison)

---

## Decision 7: Hot-Swap Measurement

**Decision:** Three complementary mechanisms for different hot-swap experiments:

### A) Version Markers for E-Swap-1 (pause duration)
Plugins set a `plugin_version` metadata field during `init()`. BenchSink detects version boundary transitions:

```rust
pub struct HotSwapRecorder {
    last_v1_time: Option<u64>,
    first_v2_time: Option<u64>,
    current_version: Option<String>,
}

// Pause duration = first_v2_time - last_v1_time
```

### B) Time-Series Throughput for E-Swap-3 (throughput dip)
Periodic throughput sampling (1ms resolution) correlated with `NodeMetrics.swap_count` atomic increment.

### C) Instrumented Orchestrator for E-Swap-6 (phase decomposition)
```rust
pub struct SwapTimeline {
    pub request_time: Instant,
    pub compile_done: Instant,
    pub instantiate_done: Instant,
    pub signal_sent: Instant,
    pub swap_acked: Instant,
    pub first_v2_output: Instant,
}
```

**Rationale:** Version markers are simple and definitive — no ambiguity about v1/v2 boundary. The phase decomposition (Marchiori pattern) reveals bottlenecks. Time-series correlation (with `swap_count`) enables the throughput-dip figure.

---

## Decision 8: Attack Scenario Measurement

**Decision:** E-Iso-1 to E-Iso-6 are automated correctness tests using `TestPipeline`. E-Iso-7 uses a parallel-branch topology where the fault node and measured node are on SEPARATE DAG branches.

**E-Iso-7 topology:**
```
BenchSource → Router → Transform_A → BenchSink_A (measured — should show <1% impact)
                     → ATTACK_NODE → Sink_B (fault path — traps)
```

**E-Iso-8 (recovery time):**
```rust
// Measure: time from trap to first successful message after recovery
let trap_time = Instant::now(); // when NodeState transitions to Recovering
// ... re-instantiation from InstancePre (~5µs) + init() ...
let resume_time = Instant::now(); // when first message exits successfully
let recovery_ns = resume_time - trap_time;
```

**Rationale:** Attack scenarios are fundamentally correctness tests (pass/fail). The only performance-relevant attack experiment (E-Iso-7) requires independent DAG branches to prove task-per-node isolation.

---

## Decision 9: Statistical Analysis — UV-Managed Python Notebooks

**Decision:** Rust handles recording (HdrHistogram + CSV). Python (via UV-managed Jupyter notebooks) handles analysis and figure generation.

**Recording output (Rust side):**
```
experiment_run/
├── config.toml              # exact config used
├── metadata.json            # hardware, versions, git SHA, timestamp
├── latency.hdr              # HdrHistogram interval log
├── throughput.csv           # periodic: timestamp,msg_count,bytes
├── per_node_metrics.csv     # node_id,messages,failures,process_ns,swap_count
├── memory.csv              # periodic: timestamp,rss_bytes
├── swap_timeline.json       # (hot-swap experiments): phase timestamps
└── sequence.csv             # seq_id,received_at_ns
```

**Analysis (Python side — UV project):**
```
eval/analysis/
├── pyproject.toml           # UV project config
├── uv.lock                  # Pinned dependencies
├── notebooks/
│   ├── 01-latency-cdf.ipynb
│   ├── 02-per-hop-overhead.ipynb
│   ├── 03-memory-scaling.ipynb
│   ├── 04-cross-arch.ipynb
│   ├── 05-hotswap-timeline.ipynb
│   ├── 06-fault-injection.ipynb
│   ├── 07-metering-decomp.ipynb
│   ├── 08-depth-scaling.ipynb
│   ├── 09-saturation.ipynb
│   ├── 10-summary-stats.ipynb
│   └── 00-warmup-validation.ipynb
└── src/wafer_analysis/
    ├── __init__.py
    ├── hdr_loader.py        # Load HdrHistogram interval logs
    ├── stats.py             # Mann-Whitney, Bootstrap, Cliff's Delta
    ├── plots.py             # Shared plot styling
    └── tables.py            # LaTeX table generation
```

**Dependencies (pyproject.toml):**
```toml
[project]
name = "wafer-analysis"
version = "0.1.0"
requires-python = ">=3.11"
dependencies = [
    "scipy>=1.14",
    "numpy>=2.0",
    "matplotlib>=3.9",
    "seaborn>=0.13",
    "pandas>=2.2",
    "hdrhistogram>=0.10",
    "statsmodels>=0.14",
    "jupyter>=1.0",
    "ipykernel>=6.0",
]
```

**Statistical method per notebook:**
1. Normality test (Shapiro-Wilk)
2. If normal → t-test; if not → Mann-Whitney U
3. Effect size: Cliff's Delta
4. Bootstrap 95% CI (10,000 resamples)
5. Report: median, IQR, p95, p99

**Workflow:**
```bash
cd eval/analysis && uv sync
uv run jupyter lab                        # interactive
uv run jupyter execute notebooks/*.ipynb  # headless (CI/reproducibility)
```

---

## Decision 10: Reproducibility Artifacts

**Decision:** Full automation suite in `eval/` directory:

```
eval/
├── configs/
│   ├── pipeline-a-telemetry.toml
│   ├── pipeline-b-inference.toml
│   ├── pipeline-c-passthrough.toml
│   ├── pipeline-c-passthrough-fuel-only.toml
│   ├── pipeline-c-passthrough-epoch-only.toml
│   ├── pipeline-c-passthrough-neither.toml
│   ├── pipeline-d-native.toml
│   ├── pipeline-depth-{1,2,3,5,10}.toml
│   └── ekuiper/
│       ├── stream.json
│       └── rule.json
├── loadgen/
│   ├── steady-500.toml
│   ├── steady-1000.toml
│   ├── steady-2000.toml
│   ├── burst.toml
│   ├── ramp.toml
│   └── hotswap-trigger.toml
├── scripts/
│   ├── setup-rpi.sh           # CPU pinning, governor, affinity, thermal
│   ├── run-experiment.sh      # Single experiment execution
│   ├── run-all.sh             # Full evaluation suite
│   └── verify-environment.sh  # Pre-flight checks
├── analysis/                  # UV-managed Python (see D9)
├── results/                   # Raw output (.gitignored, published separately)
│   └── .gitkeep
├── Makefile                   # make e-perf-1, make e-swap-1, make all, make figures
└── README.md                  # Complete reproduction instructions
```

**Published alongside thesis:**
1. Git repository (all code, configs, scripts, notebooks)
2. Raw results tarball (HdrHistogram logs + CSVs) — published to Zenodo or similar
3. Exact binary SHA256 for all .wasm modules tested
4. `Cargo.lock` pinning exact wasmtime version
5. Hardware/OS manifest (kernel version, firmware, CPU stepping)
6. `uv.lock` for exact Python analysis environment

---

## Decision 11: Warmup Detection — 30s Exclusion + ADF Verification

**Decision:** 30s warmup exclusion (messages discarded by BenchSink) with post-hoc Augmented Dickey-Fuller stationarity verification.

**Implementation:**
```rust
// BenchSink: simple time-based exclusion
fn on_message(&mut self, envelope: &RuntimeEnvelope) {
    if Instant::now() < self.warmup_until {
        self.warmup_count += 1;
        return; // don't record
    }
    // ... record to histogram ...
}
```

**Post-hoc verification (Python notebook `00-warmup-validation.ipynb`):**
```python
from statsmodels.tsa.stattools import adfuller

def verify_stationarity(latencies, window=100):
    rolling_p95 = pd.Series(latencies).rolling(window).quantile(0.95)
    result = adfuller(rolling_p95.dropna())
    assert result[1] < 0.05, f"Non-stationary! p={result[1]:.4f} — extend warmup"
```

**Rationale:** 30s at 1000 msg/s = 30,000 discarded messages. More than sufficient for Tokio stabilization and cache warming (AOT cache gives ~2ms cold start). ADF provides statistical PROOF that remaining data is stationary — unchallengeable by reviewers.

---

## Decision 12: Throughput Saturation — Ramp + Latency Threshold

**Decision:** "Saturated" = the rate where p99 latency exceeds 2× the p99 at steady-state (1000 msg/s) OR message loss exceeds 1%.

**Procedure:**
1. Establish baseline: run at 1000 msg/s → record p99 baseline
2. Run ramp profile (100→5000 msg/s over 5 min) → record latency + loss per rate band
3. Saturation point = max rate where: `p99(R) < 2 × p99(1000)` AND `loss(R) < 1%`
4. Binary search refinement between last-good and first-bad rates (30s steady runs)

**Report:** Max sustainable throughput (msg/s) per pipeline configuration.

**Rationale:** Follows Karimov 2018's "sustainable throughput" definition — max rate where latency stays bounded. The 2× threshold is objective and reproducible.

---

## Decision 13: Cross-Architecture — Same Binary, Ratio Reporting

**Decision:** Cross-compile identical binary for ARM64 (RPi 4) and x86-64. Run identical experiments. Report overhead RATIO (Wasm/Native) — dimensionless, portable across hardware.

```
ARM64:  WAFER/Native ratio = X (isolation tax on ARM)
x86-64: WAFER/Native ratio = Y (isolation tax on x86)
Target: |X - Y| < 5 percentage points
```

**Implementation:** Same Rust source, same `.wasm` plugins (platform-independent), same TOML configs. Only the target triple differs in compilation.

**Rationale:** Absolute numbers differ (x86 is faster). The ratio tells you "how much does Wasm isolation cost, regardless of hardware?" If consistent, the overhead is inherent to isolation.

---

## Decision 14: Memory Measurement — `/proc/self/statm` at 1Hz

**Decision:** Use `procfs` crate to read RSS at 1Hz. For per-node attribution (E-Perf-6), measure delta RSS when adding nodes incrementally.

```rust
use procfs::process::Process;

pub struct MemoryRecorder {
    samples: Vec<(Instant, u64)>,
}

impl MemoryRecorder {
    pub async fn sample_loop(&mut self, cancel: CancellationToken) {
        let mut ticker = tokio::time::interval(Duration::from_secs(1));
        while !cancel.is_cancelled() {
            ticker.tick().await;
            let statm = Process::myself().unwrap().statm().unwrap();
            let rss_bytes = statm.resident * procfs::page_size();
            self.samples.push((Instant::now(), rss_bytes));
        }
    }
}
```

**E-Perf-6 procedure:**
1. Pipeline with 1 node → steady-state RSS (30s average)
2. Add to 3 → RSS → delta per node
3. Add to 5 → RSS → delta
4. Add to 10 → RSS → delta
5. Per-node cost = slope of linear fit (RSS vs node count)

**Cross-platform:** On macOS (development), use `memory_stats` crate. Thesis numbers always from Linux/RPi.

---

## Decision 15: Shared Pipeline Builder — Pluggable I/O Adapters

**Decision:** One unified `PipelineBuilder` with pluggable source/sink types. `TestPipeline` and `BenchmarkPipeline` are configuration presets, not separate infrastructure.

**Source/Sink types:**

| Type | Purpose | Used by |
|------|---------|---------|
| `MemorySource` | Emit from Vec, immediate | TestPipeline (correctness) |
| `BenchSource` | Rate-controlled, timestamped, sequenced | Micro-benchmarks (E-Perf-4/6/7/8) |
| `MqttSource` | Real MQTT subscription | E2E experiments (E-Perf-1/2, E-Swap-*) |
| `CollectorSink` | Collect to Vec | TestPipeline (correctness) |
| `BenchSink` | HdrHistogram + SequenceTracker + CSV | All benchmark experiments |
| `NullSink` | /dev/null (discard) | Throughput saturation, memory measurement |

**Builder API:**
```rust
// TestPipeline (correctness):
TestPipeline::new()
    .source(MemorySource::new(messages))
    .transform("node", "plugin.wasm", config)
    .sink("output", CollectorSink::new())
    .run().await;

// BenchmarkPipeline (performance):
BenchmarkPipeline::new()
    .source(BenchSource::new(rate, payloads, warmup))
    .transform("node", "plugin.wasm", config)
    .sink("output", BenchSink::new(histogram_config))
    .run().await;
```

**Rationale:** The real orchestrator IS the system under test. Using it for measurements ensures zero code divergence. This is Tremor's model: the bench connector is just a different source+sink plugged into the same runtime.

---

## Additional Experiments (Beyond Original 16)

| ID | Experiment | RQ | Metric | Effort |
|----|-----------|-----|--------|--------|
| E-Perf-7 | Metering overhead decomposition (4 configs) | RQ1 | µs per hop × 4 configs | Low (4 TOMLs) |
| E-Perf-8 | Pipeline depth scaling (1→10 hops) | RQ1 | Latency vs hop count | Low (share with E-Perf-6) |
| E-Perf-9 | AOT cache cold vs warm start | RQ1 | ms to first message | Low (with/without cache) |
| E-Iso-8 | Recovery time (trap → resume) | RQ2 | µs from trap to first good msg | Low |
| E-Density-1 | Binary size comparison | All | KB (Wasm) vs MB (containers) | Trivial |
| E-Backpressure | Queue depth during burst | RQ1 | Queue fill time-series | Low |

---

## Experiment-to-Infrastructure Mapping (Complete)

| Experiment | Source | Sink | Load Gen | Key Measurement |
|---|---|---|---|---|
| E-Perf-1 | MqttSource | BenchSink | wafer-loadgen | HdrHistogram + throughput |
| E-Perf-2 | MqttSource | BenchSink | wafer-loadgen | HdrHistogram percentiles |
| E-Perf-3 | BenchSource | NullSink | in-process | MemoryRecorder (1Hz) |
| E-Perf-4 | BenchSource | BenchSink | in-process | NodeLatencyHistogram |
| E-Perf-5 | BenchSource | BenchSink | in-process | Ratio (ARM vs x86) |
| E-Perf-6 | BenchSource | NullSink | in-process | MemoryRecorder + incremental |
| E-Perf-7 | BenchSource | BenchSink | in-process | 4-config comparison |
| E-Perf-8 | BenchSource | BenchSink | in-process | Latency vs depth |
| E-Perf-9 | BenchSource | BenchSink | in-process | With/without cache dir |
| E-Iso-1–6 | MemorySource | CollectorSink | TestPipeline | Pass/fail assertions |
| E-Iso-7 | BenchSource | BenchSink | in-process | Throughput time-series |
| E-Iso-8 | MemorySource | CollectorSink | TestPipeline | Recovery duration |
| E-Swap-1 | BenchSource | BenchSink+HotSwapRecorder | in-process | Version boundary |
| E-Swap-2 | BenchSource | BenchSink+SequenceTracker | in-process | Gap/duplicate detection |
| E-Swap-3 | BenchSource | BenchSink | in-process | Per-second throughput |
| E-Swap-4 | BenchSource (burst) | BenchSink | in-process | Same as E-Swap-1 |
| E-Swap-5 | BenchSource | BenchSink+Collector | in-process | Correctness + throughput |
| E-Swap-6 | BenchSource | BenchSink | in-process | SwapTimeline |
| E-Val-1 | BenchSource | BenchSink | in-process | Verify injected delay visible |
| E-Backpressure | BenchSource (burst) | BenchSink | in-process | Queue depth time-series |
| E-Density-1 | — | — | — | File size measurement |
| eKuiper | MqttSource (eKuiper) | External subscriber | wafer-loadgen | Same timestamp methodology |

---

## Implementation Priority

| # | Component | Enables | Effort |
|---|-----------|---------|--------|
| 1 | `BenchSource` + `BenchSink` | All in-process experiments | ~4 hours |
| 2 | `wafer-loadgen` binary | E2E MQTT + eKuiper comparison | ~4 hours |
| 3 | Native baseline (`ProcessNode` trait) | Gap A measurement (Layer 1) | ~3 hours |
| 4 | `SwapTimeline` instrumentation | E-Swap-6 phase decomposition | ~2 hours |
| 5 | `MemoryRecorder` | E-Perf-3/6 | ~1 hour |
| 6 | `eval/` directory structure + configs | Reproducibility | ~2 hours |
| 7 | Python analysis notebooks (UV) | Thesis figures | ~8 hours |
| 8 | eKuiper SQL configs + setup | Gap B measurement | ~2 hours |
| 9 | `setup-rpi.sh` + Makefile | Automated execution | ~2 hours |

**Total: ~28 hours** (implementation, not design — design is this document)

---

## Expected Thesis Figures (22 total)

| # | Figure | Type | Experiment |
|---|--------|------|------------|
| 1 | Latency CDF comparison (Native/WAFER/eKuiper) | CDF plot | E-Perf-2 |
| 2 | Per-hop overhead vs payload size | Line + CI | E-Perf-4 |
| 3 | Per-node memory scaling | Scatter + linear fit | E-Perf-6 |
| 4 | Cross-architecture overhead ratio | Grouped bar | E-Perf-5 |
| 5 | Hot-swap timeline (time-series) | Time-series, annotated | E-Swap-1/3 |
| 6 | Pause duration distribution | Box plot | E-Swap-1 (N=50) |
| 7 | Phase decomposition (stacked bar) | Stacked bar | E-Swap-6 |
| 8 | Failed swap recovery | Time-series | E-Swap-5 |
| 9 | Pipeline throughput during fault | Time-series | E-Iso-7 |
| 10 | Metering overhead decomposition | Stacked bar (4 configs) | E-Perf-7 |
| 11 | Latency vs pipeline depth | Line + CI | E-Perf-8 |
| 12 | Throughput vs offered load (saturation) | Line, knee annotated | E-Perf-1 ramp |
| 13 | AOT cold vs warm startup | Grouped bar | E-Perf-9 |
| 14 | Queue depth during burst | Area plot | E-Backpressure |
| 15 | Recovery timeline | Timeline / sequence | E-Iso-8 |

## Expected Thesis Tables (7 total)

| # | Table | Content |
|---|-------|---------|
| 1 | Throughput comparison | Native/WAFER/eKuiper msg/s + RSS |
| 2 | Hot-swap disruption comparison | WAFER vs restart vs eKuiper restart |
| 3 | Attack scenario results | S1-S6 pass/fail + containment latency |
| 4 | Binary size comparison | Wasm plugins vs container images |
| 5 | Metering overhead by config | Absolute µs and % per mechanism |
| 6 | Pipeline depth scaling | Latency per hop count |
| 7 | Startup time (cold vs cached) | Per-plugin complexity tier |

---

## Hardware Controls (setup-rpi.sh)

```bash
#!/bin/bash
# WAFER Evaluation: RPi 4 Environment Setup
# Run before any measurement. Reversible with setup-rpi-restore.sh.

set -euo pipefail

echo "=== WAFER Evaluation Environment Setup ==="

# 1. Fix CPU frequency (disable DVFS)
echo "performance" | sudo tee /sys/devices/system/cpu/cpu*/cpufreq/scaling_governor
echo 1800000 | sudo tee /sys/devices/system/cpu/cpu*/cpufreq/scaling_max_freq
echo 1800000 | sudo tee /sys/devices/system/cpu/cpu*/cpufreq/scaling_min_freq

# 2. Disable kernel frequency throttling
sudo sh -c 'echo "force_turbo=1" >> /boot/config.txt'  # optional, requires reboot

# 3. Verify frequency is pinned
cat /sys/devices/system/cpu/cpu0/cpufreq/scaling_cur_freq  # should show 1800000

# 4. Stop non-essential services
sudo systemctl stop bluetooth hciuart cups avahi-daemon triggerhappy

# 5. Set CPU affinity for benchmark
# Core 0: load generator + MQTT broker
# Cores 1-3: WAFER runtime
echo "CPU affinity plan: loadgen+broker=core0, wafer=cores1-3"

# 6. Monitor temperature (ensure no throttling)
vcgencmd measure_temp  # should be <70°C

# 7. Disable kernel memory compaction (reduces latency jitter)
echo 0 | sudo tee /proc/sys/vm/compact_memory 2>/dev/null || true

echo "=== Environment ready. Run benchmarks with: make e-perf-1 ==="
```

---

## Threats Addressed

| Decision | Threat Avoided | Citation |
|----------|---------------|----------|
| D1 (boundary definition) | "What did you actually measure?" ambiguity | Cannata 2024 |
| D2 (unconditional) | Code divergence between benchmark and production | van der Kouwe 2018 |
| D3 (open-loop + intended time) | Coordinated omission hiding hot-swap pause | Tene 2012 |
| D4 (HdrHistogram) | Fixed-bucket precision loss at tails | Tene 2012 |
| D5 (same crate baseline) | Confounding from different runtimes | Carbone 2015 |
| D6 (same loadgen both systems) | Unfair comparison methodology | Shillaker 2020 |
| D7 (version markers) | Ambiguous swap boundary detection | Marchiori 2025 |
| D8 (parallel branches for E-Iso-7) | Measuring fault impact on same-branch node | — |
| D9 (UV + notebooks) | Non-reproducible analysis environment | Fan 2025 |
| D10 (full automation) | "Works on my machine" | Tinto 2025 |
| D11 (ADF verification) | Insufficient warmup going undetected | Georges 2007 |
| D12 (2× p99 threshold) | Undefined "saturation" | Karimov 2018 |
| D13 (ratio reporting) | Platform-specific absolute numbers | Tinto 2025 |
| D14 (/proc/self/statm) | External tools adding overhead | — |
| D15 (shared builder) | Measuring different code than production | Tremor pattern |

---

## Sources Consulted

| Source | What it informed |
|--------|-----------------|
| Tremor-rs `bench` connector (source code) | D3 (interval-based emission), D4 (sink-side HdrHistogram), D11 (warmup_secs) |
| lightbench crate (source code) | D3 (RateController), D4 (SequenceTracker), D9 (CSV export format) |
| Torvyn benchmarks (latency.rs, throughput.rs) | D15 (TestInvoker → same builder concept) |
| eKuiper source code comparators §9.3 | D6 (methodology gap — ad-hoc, no percentiles) |
| Evaluation-patterns.md (30+ papers) | All decisions (comprehensive methodology reference) |
| Tene 2012 (How NOT to Measure Latency) | D3 (open-loop), D4 (HdrHistogram), D12 (coordinated omission) |
| Georges 2007 (Statistical Rigor) | D9 (N≥30), D11 (CI stability) |
| Mytkowicz 2009 (Wrong Data) | D10 (randomized order), hardware controls |
| Karimov 2018 (Stream Benchmarks) | D3 (open-model), D12 (sustainable throughput) |
| Tinto 2025 (Wasm Migration) | D13 (dimensionless ratios), hardware controls (SCHED_FIFO, pinned freq) |
| Marchiori 2025 (Wasm Orchestration) | D7 (phase decomposition reveals bottlenecks) |
| Carbone 2015 (Flink Snapshots) | D5 (same runtime for baseline) |
| Delmonte 2020 (Rhino) | D3 (separate load generator), D7 (phase decomposition) |
| Kjorveziroski 2023 (Wasm vs Containers) | D9 (100 reps + Mann-Whitney U) |
| Zhang 2025 (Wasm Runtimes Survey) | New experiments (pipeline depth gap) |
| HdrHistogram Rust crate docs | D4 (API: new_with_bounds, record, value_at_quantile) |
| procfs Rust crate docs | D14 (StatM::resident, page_size) |
| RPi documentation (frequency-management.adoc) | Hardware controls (DVFS, governors) |
| Zenodo replication dataset (Async/Await pipeline) | D9 (30 runs × CSV + HDR format) |
| stats-claw / anofox-statistics crates | D9 (Rust stats available but Python preferred for thesis) |
| Cannata 2024 (SliceGuard) | D1 (include serialization in measurement) |
| Shillaker 2020 (FAASM) | D6 (same application code both platforms) |

---

## Amendments to Prior Sessions

### Amendment to Session 5 (Orchestrator) — ProcessNode Trait

The node loop pseudocode from Session 5 D3 gains a `ProcessNode` trait abstraction that allows both Wasm and native implementations. The loop body remains identical — only the `transform.process()` call dispatches differently based on the concrete type.

### Amendment to Session 6 (Plugin Rewrite) — BenchSource/BenchSink

Session 6 D6's `TestPipeline` concept is generalized to a shared `PipelineBuilder` that accepts different source/sink implementations. `TestPipeline` becomes a convenience constructor, not a separate infrastructure.

### Amendment to Session 7 (Performance) — D8 Benchmark Harness

Session 7 D8's "benchmark-first strategy" is now fully specified. The "4-8 hours" estimate for benchmark harness becomes the detailed breakdown in this document's Implementation Priority section (~28 hours total including analysis tooling).

### No amendments to Sessions 1–4

All decisions from Sessions 1 (WIT), 2 (host runtime), 3 (node types), and 4 (config schema) remain unchanged.
