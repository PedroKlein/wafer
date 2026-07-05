---
name: wafer-project
description: >
  Always-loaded project context for WAFER (WebAssembly Flow Execution Runtime). Provides
  identity, thesis contribution framing, research questions with pass criteria, architectural
  invariants, scope qualifiers, conventions, comparator positioning, and what WAFER is NOT.
  Every agent interaction starts with this mental model. Do NOT load other skills for this
  information — it is always available. Triggers: always loaded via project .agents/ discovery.
alwaysLoaded: true
---

# WAFER — Project Context

## NEVER Assume (Common LLM Mistakes)

- **NEVER describe hot-swap as state-preserving** — Invariant 7: state lives inside the WASM
  instance and is lost on every swap; this is by design (stateless transforms), not a limitation
- **NEVER elevate one feature as "the primary novelty"** — the integrated system is the
  contribution; framing any single feature (isolation OR hot-swap OR performance) as "the main
  point" misrepresents the thesis
- **NEVER position WAFER as competitive for arbitrary computation** — "competitive performance"
  means within 30% of eKuiper throughput on telemetry pipeline workloads specifically
- **NEVER suggest distributed deployment, K8s, or multi-process** — architectural invariant #1
  is non-negotiable; the single-process constraint is the thesis's edge viability claim
- **NEVER use domain terms without scope qualifiers** — "pipeline", "isolation", "hot-swap",
  "edge gateway" all have precise meanings (see Scope Qualifiers table); unqualified usage
  confuses readers unfamiliar with the streaming/edge domain

## Applying This Context

| Task type | Priority sections | Hard constraint |
|-----------|------------------|-----------------|
| Writing thesis prose | Scope Qualifiers → Comparators | Terms MUST use qualifiers |
| Reviewing code | Architectural Invariants #1–10 | NEVER violate any invariant |
| Designing experiments | RQ pass criteria + method | N≥30, open-loop, Mann-Whitney U |
| Hot-swap implementation | Invariant #8 + "NOT stateful" | Swap is stateless — state is lost |
| Benchmarking | RQ1 pass criteria + hardware | Paired baselines: native + eKuiper |
| Adding features | "What WAFER Is NOT" + Invariants | Don't add windowing, exactly-once, etc. |

---

## Identity

**WAFER** (WebAssembly Flow Execution Runtime) — Pedro Klein's undergraduate thesis
(TCC/TG2) at UFRGS, Porto Alegre, Brazil. A **single-process Rust runtime** that executes
typed DAGs of WebAssembly components on IoT edge gateways.

**Thesis statement** (v3, 2026-06-12): WAFER targets the gap between cloud-managed
platforms (Azure IoT Operations: 16+ GB Kubernetes) and monolithic edge processors
(eKuiper: no isolation, no live update). The contribution is the **runtime architecture
itself** — demonstrating that typed Wasm components deliver viable performance, fault
containment, and live evolvability on constrained gateways.

**The runtime IS the contribution.** No single feature is elevated as "the primary novelty."
The novelty is the *integrated system* that simultaneously achieves all properties below
on sub-8GB gateway hardware.

---

## Research Questions (v3 — CURRENT)

| RQ | Question | Pass Criterion |
|----|----------|----------------|
| **RQ1** | What is the performance cost of typed Wasm boundaries on edge hardware? | Within 30% of eKuiper throughput; p95 within 2× eKuiper; <50µs per-hop on RPi 4 |
| **RQ2** | Do per-stage sandboxes contain faults without pipeline-wide failure? | All 6 attack scenarios (S1-S6) contained; <1% throughput impact on healthy stages |
| **RQ3** | What is the disruption cost of replacing a stage at runtime? | <100ms pause at p95; zero message loss; <5% throughput dip vs full restart's 100% |

**Evaluation architecture** (three-way comparison):
```
Native Rust (ceiling) ←── Gap A: "isolation tax" ──→ WAFER ←── Gap B: "competitive?" ──→ eKuiper
```

**Hardware**: Raspberry Pi 4 (primary), Jetson Orin (inference), x86 (cross-validation).
**Method**: N≥30 repetitions, open-loop load gen, Mann-Whitney U, HdrHistogram, Bootstrap CI95.

---

## Architectural Invariants (NEVER violate)

1. **Single-process** — all nodes in one OS process; no IPC, no containers, no K8s
2. **DAG-only** — cycles rejected at build time (toposort = cycle detection)
3. **SPSC queues** — every edge is single-producer single-consumer; fan-in requires Joiner
4. **All queues bounded** — no unbounded channels; backpressure propagates end-to-end
5. **Sources/sinks are native Rust** — only transforms/routers/joiners are WASM (ADR-0004)
6. **WIT-typed boundaries** — all WASM calls through `pipeline:transform@0.1.0` WIT contracts
7. **Stateless transforms** — no host-managed state; state lives inside WASM instance (lost on swap)
8. **Drain-and-flip** — hot-swap: stop routing → drain in-flight → flip → retire old
9. **Processing-time only** — no event-time, no watermarks, no windows
10. **Fuel/epoch metering** — untrusted plugins bounded in computation AND wall-clock time

---

## Scope Qualifiers (MUST appear consistently in writing)

| Term | Means | Does NOT Mean |
|------|-------|---------------|
| "Edge gateway" | Linux-capable, ≥4GB RAM (RPi 4, Jetson) | Microcontrollers (Cortex-M) |
| "Isolation" | Memory containment + capability scoping | Information-flow control, covert-channel elimination |
| "Hot-swap" | Stateless node replacement | State-preserving live update |
| "Competitive performance" | Within 30% of eKuiper throughput | Near-native for arbitrary computation |
| "Pipeline" | Stateless transform DAG (parse, filter, route) | Full stream processor with windowing/exactly-once |

---

## Conventions

| Convention | Detail |
|-----------|--------|
| WASM target | `wasm32-wasip2` (Component Model, WASI Preview 2) |
| Rust edition | 2024 (stable channel, `rust-version = "1.85"`) |
| Pipeline config | TOML only |
| WIT package | `pipeline:transform@0.1.0` in `/wit/` |
| Plugin structure | `/plugins/{name}/src/lib.rs`, `crate-type = ["cdylib"]` |
| Workspace | `wafer-core` (lib), `wafer-runtime` (bin), `wafer-types` (lib), `waferctl` (bin) |
| Toolchain | `rust-toolchain.toml` (stable channel, Edition 2024) |
| Command runner | `just` (justfile at root) |
| ADRs | `docs/adr/NNNN-<slug>.md`, Michael Nygard format, 6 accepted |
| Overflow policies | `slow` (backpressure), `drop`, `dead-letter` |
| Node categories | Source, Transform, Router, Joiner, Sink |
| Metrics | `prometheus-client` crate, separate port |
| Logging | `tracing` + `EnvFilter` |
| Control plane | axum HTTP REST API |
| Plugin sources | Local (`plugin_path`) or OCI (`plugin_ref` via `oci-client`) |

---

## Comparator Positioning (Validated by 12-Repo Systematic Review)

| System | Shares With WAFER | Lacks vs WAFER |
|--------|-------------------|----------------|
| **eKuiper** | Same HW (RPi), same protocol (MQTT), lightweight, goroutine-per-op | No per-operator isolation, no typed contracts, full rule restart |
| **Torvyn** | Rust+Wasmtime+WIT+typed streams+backpressure | No hot-swap, no IoT protocols, no edge HW benchmarks, single-task model |
| **Spin** | Component Model, Tokio, InstancePre, Factors, pooling | Serverless (no streaming, no DAG, no backpressure), whole-app reload |
| **Wassette** | Component Model, deny-by-default, InstancePre, OCI | No streaming, no DAG, no backpressure, atomic replace (no drain) |
| **Azure IoT Ops** | Wasm+WIT+DAG, same domain, edge IoT | Platform-managed (K8s), linear only, no hot-swap, no standalone |
| **Fluvio** | Wasm in data path, Rust, streaming | Core modules (not CM), shared Store, no isolation, eventual-consistency reload |
| **Flow-Like** | Wasm+DAG, AOT caching, epoch+fuel | One-shot workflows (not streaming), no isolation, no hot-swap |
| **Tremor** | Rust DAG, streaming, backpressure, 10TB/day | No Wasm isolation, no typed plugin contracts, no per-node hot-swap |
| **Wick** | Wasm components, flow DAG, reactive streams | Custom WasmRS (obsolete), no backpressure, abandoned (single-maintainer risk) |
| **Node-RED** | Flow DAG, edge deployment, huge ecosystem | Zero isolation (CVE-2025-41656), no typing, JS single-thread |

**Industry Validation Chain** (12-repo review confirmed):
1. **Bytecode Alliance (wasmtime)** — Component Model runtime foundation ✓
2. **Fermyon (Spin)** — Component Model at scale, Factors architecture ✓
3. **Microsoft (Wassette)** — Security-focused CM, deny-by-default ✓
4. **Azure (Dataflow Graphs)** — WIT operators in DAG at edge, wasm32-wasip2 ✓
5. **eKuiper (LF Edge)** — Production edge streaming, published RPi numbers ✓

**Uniqueness confirmed**: No existing system combines ALL of: per-node WIT-typed isolation + per-node
drain-and-flip hot-swap + bounded backpressure + continuous streaming + measured on ≤4GB ARM.

### Positioning Framing (for Thesis Writing)

- vs eKuiper: "What does per-operator isolation cost?" — not "faster than native"
- vs Spin: "Spin demonstrates CM viability for serverless; WAFER extends CM to continuous edge streaming"
- vs Azure: "WAFER provides equivalent data processing as a standalone runtime with explicit backpressure and hot-swap"
- vs Torvyn: "WAFER adds per-stage fault isolation and IoT-protocol integration"
- vs Wick (abandoned): validates narrow scope + standard protocols + clear contribution criteria
- vs Fluvio: validates Wasm-in-data-path at scale; WAFER adds persistent DAG with isolation

---

## What WAFER Is NOT

- **NOT distributed** — single node, single process, single machine
- **NOT event-time aware** — no watermarks, no windows, no out-of-order handling
- **NOT stateful stream processing** — no checkpoints, no savepoints, no exactly-once
- **NOT a Kubernetes operator** — no CRDs, no pods, no orchestration layer
- **NOT multi-tenant** — single pipeline, single trust domain (plugins are untrusted)
- **NOT a general-purpose FaaS** — continuous streaming DAG, not request/response

---

## Use Cases

| UC | Pipeline | Domain |
|----|----------|--------|
| **UC1** | MQTT → JSON Parse → Threshold Filter → Router → Alert/Log Sinks | Industrial sensor telemetry gateway |
| **UC2** | MQTT → Tensor Prep → MNIST/MobileNet (wasi-nn) → Result Format → MQTT | Edge ML inference for visual inspection |

---

## Reference Documents

Paths are relative to the workspace root (`~/Dev/github.com/PedroKlein/`).
Do NOT load all references at once — each is 5,000–15,000 words. Load only the
one directly relevant to the current task.

| Need | Read |
|------|------|
| Full design specification | `wafer-poc/main/docs/SPEC.md` |
| Implementation status | `wafer-poc/main/docs/MVP.md` |
| Architectural decisions | `wafer-poc/main/docs/adr/` (6 ADRs) |
| Current thesis statement & RQs | `tcc-doc/main/research/analysis/thesis-statement-v3.md` |
| Evaluation plan (experiments, stats, threats) | `tcc-doc/main/research/analysis/evaluation-plan.md` |
| Comparator positioning matrix | `tcc-doc/main/research/analysis/positioning-matrix.md` |
| Use cases with trade-offs | `tcc-doc/main/context/use-cases.md` |
| Literature synthesis (75 papers) | `tcc-doc/main/findings/synthesis-findings.md` |
| Similar systems deep search | `tcc-doc/main/findings/similar-systems-2025-2026.md` |
| API endpoints | `wafer-poc/main/docs/api.md` + `docs/api/openapi.yaml` |
