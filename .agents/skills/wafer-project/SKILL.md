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

> **Ground truth rule.** The `docs/` tree has been refactored into arc42-lite
> (`docs/architecture/`, `docs/rfcs/`, `docs/adr/`, `docs/interfaces/`,
> `docs/operations/`, `docs/status/`). When docs and implementation disagree,
> trust source code, WIT files, and `docs/status/implementation-gaps.md`.

## NEVER Assume (Common LLM Mistakes)

- **NEVER describe hot-swap as state-preserving** — Invariant 7: state lives inside the WASM
  instance and is lost on every swap; this is by design (stateless transforms), not a limitation
- **NEVER elevate one feature as "the primary novelty"** — the *integrated runtime* is the
  contribution; framing any single feature (isolation OR hot-swap OR performance) as "the main
  point" misrepresents the thesis
- **NEVER position WAFER as competitive for arbitrary computation** — "competitive performance"
  means within 30% of eKuiper throughput on telemetry pipeline workloads specifically
- **NEVER suggest distributed deployment, K8s, or multi-process** — architectural invariant #1
  is non-negotiable; the single-process constraint is the thesis's edge viability claim
- **NEVER use domain terms without scope qualifiers** — "pipeline", "isolation", "hot-swap",
  "edge gateway" all have precise meanings (see Scope Qualifiers table); unqualified usage
  confuses readers unfamiliar with the streaming/edge domain
- **NEVER mention Joiner as a node type** — Joiner was removed in Session 3 of the refactor;
  fan-in is now an implicit host topology (multi-producer mpsc into one consumer). The five
  node categories are exactly: Source, Sink, Transform, Filter, Router.

## Applying This Context

| Task type | Priority sections | Hard constraint |
|-----------|------------------|-----------------|
| Writing thesis prose | Scope Qualifiers → Comparators | Terms MUST use qualifiers |
| Reviewing code | Architectural Invariants #1–10 | NEVER violate any invariant |
| Designing experiments | RQ pass criteria + method | N≥30, open-loop, Mann-Whitney U |
| Hot-swap implementation | Invariant #8 + "NOT stateful" | Watch-channel swap is stateless — state is lost |
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
3. **Bounded mpsc queues** — every edge is a bounded tokio `mpsc` channel; fan-in is
   implicit host topology (multiple producers → one consumer). No Joiner node type.
4. **All queues bounded** — no unbounded channels; backpressure propagates end-to-end
5. **Sources/sinks are native Rust** — only Transform, Filter, and Router are WASM (ADR-0004)
6. **WIT-typed boundaries** — all WASM calls go through the four `pipeline:*@0.1.0` packages
   (`pipeline:types`, `pipeline:node`, `pipeline:routing`, `pipeline:host`)
7. **Stateless transforms** — no host-managed state; state lives inside the WASM instance
   and is lost on every hot-swap
8. **Watch-channel hot-swap** — swap happens *between* messages: the runner selects between
   the input queue and a `watch::Receiver<Option<SwapPayload>>`; when a swap arrives, the
   old instance drops and the new pre-instantiated component takes over. No 4-phase drain.
9. **Processing-time only** — no event-time, no watermarks, no windows
10. **Fuel/epoch metering** — untrusted plugins bounded in computation (fuel) and wall-clock
    time (epoch ticker on a dedicated OS thread)

---

## Scope Qualifiers (MUST appear consistently in writing)

| Term | Means | Does NOT Mean |
|------|-------|---------------|
| "Edge gateway" | Linux-capable, ≥4GB RAM (RPi 4, Jetson) | Microcontrollers (Cortex-M) |
| "Isolation" | Memory containment + capability scoping | Information-flow control, covert-channel elimination |
| "Hot-swap" | Stateless node replacement between messages | State-preserving live update |
| "Competitive performance" | Within 30% of eKuiper throughput | Near-native for arbitrary computation |
| "Pipeline" | Stateless transform DAG (parse, filter, route) | Full stream processor with windowing/exactly-once |

---

## Conventions

| Convention | Detail |
|-----------|--------|
| WASM target | `wasm32-wasip2` (Component Model, WASI Preview 2) |
| Rust edition | 2024 (stable channel, `rust-version = "1.85"`) |
| Pipeline config | TOML only |
| WIT packages | `pipeline:types@0.1.0`, `pipeline:node@0.1.0`, `pipeline:routing@0.1.0`, `pipeline:host@0.1.0` in `/wit/` |
| WIT worlds | `transform-node`, `filter-node`, `inference-node` (in `pipeline:node`); `router-node` (in `pipeline:routing`) |
| Envelope shape | `Arc<EnvelopeHeader>` (metadata) + `Bytes` payload + `Lineage` trail; buffer resource with `borrow<buffer>` for zero-copy input |
| Plugin structure | `/plugins/{name}/src/lib.rs`, `crate-type = ["cdylib"]` |
| Workspace crates | `wafer-core` (runtime lib), `wafer-runtime` (bin), `wafer-types` (shared types), `wafer-config` (loader + validation), `wafer-plugin` (guest SDK), `wafer-loadgen` (eval harness), `waferctl` (CLI) |
| Toolchain | `rust-toolchain.toml` (stable channel, Edition 2024) |
| Tool manager / command runner | Rust via rustup + `rust-toolchain.toml`; non-Rust tools/tasks via `mise.toml`; run `mise run setup` for helper tools and `mise tasks ls` for tasks |
| ADRs | `docs/adr/NNNN-<slug>.md`, Michael Nygard format |
| RFCs | `docs/rfcs/RFC-NNN-<slug>.md` (authoritative long-form decision archive) |
| Error categories | 5-variant enum: `bad-input`, `dependency-failed`, `processing-failed`, `timed-out`, `unrecoverable` |
| Overflow policies | `slow` (backpressure), `drop`, `dead-letter` |
| Node categories | Source, Sink, Transform, Filter, Router (no Joiner) |
| Node config field | Single `plugin` field on `WasmNodeDef` (not `plugin_path` / `plugin_ref`); the loader dispatches to local path vs OCI reference based on the value |
| Edge field | Single `port` field on `EdgeDef` (only used for router outputs); no `from_port` / `to_port` |
| Metrics | `prometheus-client` crate, separate port |
| Logging | `tracing` + `EnvFilter` |
| Control plane | axum HTTP REST API |

---

## Comparator Positioning (Validated by 12-Repo Systematic Review)

| System | Shares With WAFER | Lacks vs WAFER |
|--------|-------------------|----------------|
| **eKuiper** | Same HW (RPi), same protocol (MQTT), lightweight, goroutine-per-op | No per-operator isolation, no typed contracts, full rule restart |
| **Torvyn** | Rust+Wasmtime+WIT+typed streams+backpressure, edge-processing use case published, Apache-2.0 | No per-node hot-swap (task-per-flow reactor), no MQTT/HTTP adapters (protocol-agnostic), no canonical Pi/Jetson benchmarks |
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

**Uniqueness confirmed**: No existing system combines ALL of: per-node WIT-typed isolation +
per-node between-messages hot-swap + bounded backpressure + continuous streaming + measured
on ≤4GB ARM.

### Positioning Framing (for Thesis Writing)

- vs eKuiper: "What does per-operator isolation cost?" — not "faster than native"
- vs Spin: "Spin demonstrates CM viability for serverless; WAFER extends CM to continuous edge streaming"
- vs Azure: "WAFER provides equivalent data processing as a standalone runtime with explicit backpressure and hot-swap"
- vs Torvyn: "WAFER adds per-node hot-swap, IoT-protocol integration, and constrained-hardware evaluation" (Torvyn = public Rust runtime, not a UFRGS thesis; edge processing IS a published Torvyn use case)
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
- **NOT a joiner runtime** — fan-in is implicit host topology (multi-producer mpsc), not a
  first-class Wasm node type

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

Because the doc tree is being restructured (see banner at top), some of these targets
are being replaced. During the migration, prefer the source code / WIT files as the
authoritative view of *what is*, and RFCs (`docs/rfcs/`) or decision docs as
authoritative for *why*.

| Need | Read |
|------|------|
| Architecture overview (post-migration) | `wafer-poc/main/docs/architecture/` |
| Implementation status (post-migration) | `wafer-poc/main/docs/status/implementation-status.md` |
| WIT contract reference (post-migration) | `wafer-poc/main/docs/interfaces/wit-contracts.md` |
| HTTP API reference (post-migration) | `wafer-poc/main/docs/interfaces/http-api.md` |
| Config schema reference (post-migration) | `wafer-poc/main/docs/interfaces/config-schema.md` |
| RFC archive (design decisions) | `wafer-poc/main/docs/rfcs/` |
| Architecture Decision Records | `wafer-poc/main/docs/adr/` |
| Ground-truth WIT contracts | `wafer-poc/main/wit/*.wit` (four packages) |
| Current thesis statement & RQs | `tcc-doc/main/research/analysis/thesis-statement-v3.md` |
| Evaluation plan (experiments, stats, threats) | `tcc-doc/main/research/analysis/evaluation-plan.md` |
| Comparator positioning matrix | `tcc-doc/main/research/analysis/positioning-matrix.md` |
| Use cases with trade-offs | `tcc-doc/main/context/use-cases.md` |
| Literature synthesis (75 papers) | `tcc-doc/main/findings/synthesis-findings.md` |
| Similar systems deep search | `tcc-doc/main/findings/similar-systems-2025-2026.md` |
