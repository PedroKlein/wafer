# WAFER — Implementation TODOs

> Tasks required to make the runtime evaluation-ready.  
> Priority: 🔴 Critical path | 🟡 Important | 🟢 Nice-to-have

---

## Sibling Repos

| Repo | pi-repos ID | What it is |
|------|-------------|------------|
| **tcc-doc** | `github.com/PedroKlein/tcc-doc` | Research, evaluation plan, thesis writing, RQ definitions |
| **Obsidian vault** | `github.com/PedroKlein/obsidian-personal` | Knowledge base — 252 literature notes under `TCC/` |

> All `tcc-doc/...` references below are relative to that repo's root.
> Use `repos_info` or the pi-repos group context to resolve actual filesystem paths.
> For the master index of which document is authoritative, read `tcc-doc/SOURCES-OF-TRUTH.md`.

---

## Cross-References (Context for AI)

| Document | Location | What it contains |
|----------|----------|------------------|
| **Evaluation Plan** | `tcc-doc/research/analysis/evaluation-plan.md` | Full methodology: 16 experiments, metrics, statistical rigor, hardware setup, threats to validity, expected results, figures |
| **Research Questions** | `tcc-doc/research/analysis/thesis-statement-v3.md` | 3 RQs with pass/fail criteria: RQ1 (performance), RQ2 (isolation), RQ3 (hot-swap) |
| **Use Cases** | `tcc-doc/context/use-cases.md` | UC1 (telemetry gateway), UC2 (edge inference) — defines pipeline topologies |
| **Contributions** | `tcc-doc/research/contributions.md` | What WAFER claims, what it doesn't, positioning vs 8 comparators |
| **Counter-Arguments** | `tcc-doc/research/analysis/counter-args-triage.md` | 18 serious challenges to WAFER's claims + mitigations |
| **eKuiper Analysis** | Obsidian `TCC/software/ekuiperEKuiperDocs2025.md` | eKuiper architecture, limitations, why their Wasm was removed |
| **Source-Code Comparators** | `tcc-doc/findings/source-code-comparators.md` | 9 systems reviewed at code level (Torvyn, Fluvio, Spin, AIO, Tremor, Wick, eKuiper, Wassette, Flow-Like) |
| **WAFER SPEC** | `docs/SPEC.md` §15 | Original evaluation plan (superseded by tcc-doc version for methodology details, but hardware/scenario definitions still valid) |
| **Existing Benchmarks** | `docs/benchmarks/hot-swap.md` | Hot-swap prepare phase: ~9ms on Apple Silicon (PASS). Phase breakdown measured. |
| **ADR-0003** | `docs/adr/0003-drain-and-flip-hotswap.md` | Design rationale for hot-swap mechanism |

> **Key principle:** `tcc-doc/research/analysis/evaluation-plan.md` is the authoritative source for
> HOW experiments should be run (statistical method, N=30, open-loop, HdrHistogram, warmup, etc.).
> This TODO tracks WHAT needs to be built to enable those experiments.
>
> **Start here:** `tcc-doc/SOURCES-OF-TRUTH.md` — declares which document is authoritative for each topic.
> Also see `tcc-doc/RQ-VERSION-MAP.md` if you encounter references to RQ4/5/6 (old scheme).

---

## Phase 1: Evaluation Infrastructure

### 🔴 Open-Loop Load Generator
- [ ] Rust binary: constant-arrival-rate MQTT publisher (Tokio interval)
- [ ] HdrHistogram recording (per-message latency from intended-publish-time)
- [ ] Configurable: rate (msg/s), payload size, duration, burst pattern
- [ ] Open-loop (NOT closed-loop) — avoids coordinated omission
- [ ] Output: HdrHistogram dump + CSV + summary stats
- [ ] Payload templates: 120B JSON telemetry, 1KB, 10KB, 100KB

### 🔴 Native Rust Baseline (Pipeline D)
- [ ] Same logic as Pipeline A (JSON parse → threshold filter → content router)
- [ ] Same Tokio channels (bounded mpsc), same message envelope
- [ ] No Wasm, no WIT boundary — plain Rust functions
- [ ] Same Prometheus metrics endpoint for fair comparison
- [ ] Same MQTT source/sink (rumqttc)

### 🔴 eKuiper Comparison Setup
- [ ] eKuiper Docker/native install on RPi 4
- [ ] Equivalent SQL rule: `SELECT * FROM mqtt_stream WHERE temperature > 50`
- [ ] Match MQTT topics and payload format exactly
- [ ] Document version + configuration for reproducibility

### 🔴 Attack Scenario Wasm Modules (S1–S6)
- [ ] S1: Buffer overflow (write beyond linear memory bounds)
- [ ] S2: Cross-node memory read attempt (impossible by design — document why)
- [ ] S3: Infinite loop (CPU exhaustion → epoch interrupt)
- [ ] S4: Excessive memory allocation (exceed `max_memory` config)
- [ ] S5: Unauthorized filesystem access (attempt open() without capability)
- [ ] S6: Panic/abort within transform (verify pipeline continues)

### 🟡 Per-Message Latency Instrumentation
- [ ] Verify/add HdrHistogram integration in wafer-runtime
- [ ] Timestamp at MQTT receive → timestamp at sink publish = E2E latency
- [ ] Exportable as HdrHistogram dump file per run
- [ ] Separate from Prometheus (which scrapes at intervals)

### 🟡 Pipeline Configs for Scaling Test
- [ ] 1-node pass-through (baseline)
- [ ] 3-node chain (source → transform → sink)
- [ ] 5-node chain (Pipeline A topology)
- [ ] 10-node chain (stress test)
- [ ] Same payload (120B JSON) for all

### 🟡 Variable Payload Pass-Through
- [ ] Config or parameterized test: 100B, 1KB, 10KB, 100KB payloads
- [ ] Measure per-hop WIT boundary cost as f(payload_size)

### 🟡 Message Accounting
- [ ] Counter: messages received at source
- [ ] Counter: messages delivered to sink
- [ ] Delta check after hot-swap: lost = 0, duplicated = 0
- [ ] Exposed via metrics endpoint or log at shutdown

---

## Phase 2: Experiment Execution

### 🔴 RPi 4 Environment Setup
- [ ] Flash RPi OS 64-bit, pin kernel version
- [ ] Disable CPU frequency governor (set `performance`)
- [ ] Document: background services, kernel version, firmware
- [ ] Install Rust toolchain + cross-compile setup
- [ ] Set up MQTT broker (mosquitto) on dedicated core (taskset)
- [ ] Network: load generator on separate machine or isolated cores

### 🔴 Run Experiments (in order from eval-plan)
- [ ] E-Val-1: Inject 50ms delay, verify in p99 (methodology validation)
- [ ] E-Perf-4: Per-hop overhead (pass-through, variable payload)
- [ ] E-Perf-6: Per-node RSS (1/3/5/10 nodes)
- [ ] E-Perf-1: Throughput (Pipeline A — WAFER vs Native vs eKuiper)
- [ ] E-Perf-2: Latency under load (same pipeline, 1000 msg/s)
- [ ] E-Perf-5: Cross-architecture (same binary on x86)
- [ ] E-Swap-6: Phase decomposition (prepare/drain/swap/resume timing)
- [ ] E-Swap-1: Pause duration under 1000 msg/s
- [ ] E-Swap-2: Message accounting (zero loss)
- [ ] E-Swap-3: Throughput dip during swap
- [ ] E-Swap-4: Swap under 2× burst
- [ ] E-Swap-5: Failed swap recovery (v2 traps immediately)
- [ ] E-Iso-1 to E-Iso-7: All attack scenarios + pipeline continuity

### 🟡 Jetson Inference Benchmarks
- [ ] MNIST micro-benchmark: wasi-nn overhead isolation
- [ ] (Optional) MobileNetV2 pipeline if time permits
- [ ] CPU vs GPU comparison on same binary

### 🟡 Automation & Reproducibility
- [ ] Shell script / Makefile to run full benchmark suite
- [ ] Ansible playbook or setup script for RPi environment
- [ ] Raw data output directory structure
- [ ] Python/R analysis scripts for stats + plots

---

## Phase 3: Polish (after experiments)

### 🟢 Optional Enhancements
- [ ] MobileNetV2 inference pipeline (UC2 narrative strengthening)
- [ ] OCI registry benchmark (load from ghcr.io vs local)
- [ ] Multi-pipeline memory isolation test (if multi-pipeline supported)

---

## Done ✅

- [x] Core DAG orchestration (petgraph)
- [x] 9 Wasm plugins (pass-through, filter, json-parse, content-router, merge-joiner, tensor-prep, mnist-inference, result-format, uppercase)
- [x] Hot-swap drain-and-flip mechanism (with SwapMetrics phase timing)
- [x] Bounded SPSC queues with overflow policies (slow/drop/dead-letter)
- [x] HTTP control plane (axum) + Prometheus metrics
- [x] OCI registry support (ghcr.io)
- [x] Fuel-based metering (epoch interruption)
- [x] WASI Preview 2 support
- [x] WASI-NN MNIST inference (CPU + CUDA on Jetson)
- [x] waferctl CLI
- [x] Criterion benchmarks (throughput, hot_swap, metrics)
- [x] Integration tests (MQTT, pipeline lifecycle)
- [x] DAG topology examples (chain, diamond, fanout, filter, etc.)
