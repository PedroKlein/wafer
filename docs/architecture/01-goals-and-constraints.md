# Goals and Constraints

> arc42 §1–3: driving forces, quality goals, stakeholders, and hard architectural constraints.

## System purpose

WAFER (WebAssembly Flow Execution Runtime) is a single-process DAG pipeline runtime for edge IoT gateways. It composes pipeline stages as typed, sandboxed WebAssembly Component Model modules connected by bounded queues with backpressure.

The runtime IS the contribution. No single feature — performance, isolation, or hot-swap — stands above the others. The novelty is the integrated system that simultaneously achieves typed DAG composition, per-stage fault isolation, and live stage replacement on sub-8 GB gateway hardware. No existing system provides this combination.

## Research questions and pass criteria

The evaluation validates that each architectural property holds in practice on constrained hardware (Raspberry Pi 4, Jetson Orin Nano).

### RQ1: What is the performance cost of typed Wasm boundaries on edge hardware?

Does a Wasm-isolated pipeline achieve competitive throughput and latency compared to an established edge stream processor on the same constrained hardware?

| Sub-question | Metric | Pass criterion |
|---|---|---|
| RQ1a | µs per WIT boundary crossing (empty pass-through) | < 50 µs on RPi 4 |
| RQ1b | Pipeline throughput at saturation (4-stage) | Within 30 % of eKuiper |
| RQ1c | p95/p99 tail latency under sustained 1000 msg/s | p95 within 2× eKuiper |
| RQ1d | RSS for 5-node pipeline | < 150 MB total; < 10 MB per added node |
| RQ1e | AOT compile + instantiate time | < 50 ms on RPi 4 |

Baselines: native Rust (same logic, no Wasm — measures "isolation tax") and eKuiper (Go-based edge stream processor — measures "competitive viability").

### RQ2: Do per-stage sandboxes contain faults without pipeline-wide failure?

Do Wasm capability-scoped boundaries contain misbehaving nodes without affecting healthy stages or host integrity?

| Sub-question | Metric | Pass criterion |
|---|---|---|
| RQ2a | Memory isolation (buffer overflow, cross-stage read, OOB) | All trapped |
| RQ2b | CPU exhaustion (infinite loop) | Epoch interrupt within configured budget |
| RQ2c | Resource limits (excess allocation, unauthorized FS/net) | Trap before damage |
| RQ2d | Throughput of healthy stages during fault | < 1 % drop |
| RQ2e | Cross-contamination (state leakage after crash) | Zero leakage |

Attack scenarios S1–S6 implemented as six dedicated plugins under `plugins/attacks/`.

### RQ3: What is the disruption cost of replacing a stage at runtime?

Does hot-swap achieve bounded pause duration and zero message loss when replacing a node under sustained load?

| Sub-question | Metric | Pass criterion |
|---|---|---|
| RQ3a | Pause duration (last msg v1 → first msg v2) | < 100 ms at p95 |
| RQ3b | Message accounting (loss + duplication) | Zero loss, zero duplication |
| RQ3c | Throughput dip during swap window | < 5 % vs full restart's 100 % |
| RQ3d | Swap under 2× burst | Still < 100 ms |
| RQ3e | Failed swap recovery (v2 traps on first message) | Pipeline survives, 0 messages lost |

Baselines: full pipeline restart (naive) and eKuiper rule restart.

## Stakeholders

| Stakeholder | Concern | Addressed by |
|---|---|---|
| **Thesis reader / evaluator** | Reproducible evidence that the three RQ pass criteria hold | Evaluation harness, quantitative benchmarks, documented methodology |
| **Edge-gateway operator** | 24/7 uptime on constrained hardware with safe updates | Single-process deployment, hot-swap, bounded memory, DLQ for failures |
| **Plugin author** | Ship logic in any Component-Model language without knowing the host | WIT-typed contracts, `wafer-plugin` guest SDK, per-node capability scoping |

## Quality goals (ranked)

1. **Fault containment** — a misbehaving plugin must not crash the host or corrupt sibling nodes.
2. **Bounded resource consumption** — memory and CPU usage must remain predictable regardless of plugin behaviour.
3. **Competitive throughput** — the pipeline must stay within 30 % of eKuiper for equivalent workloads.
4. **Operational evolvability** — individual stages must be replaceable at runtime without pipeline downtime.
5. **Simplicity of deployment** — a single binary, a single TOML file, no container orchestrator.

## Hard architectural constraints

These are non-negotiable boundaries. They are facts of the current system, not aspirations.

### C1: Single process

The entire pipeline — sources, Wasm stages, sinks, control plane — runs in one OS process. There is no inter-process communication, no sidecar, no container-per-node. This is a deliberate design choice that maximises efficiency on gateway-class hardware (4 GB RAM, quad-core ARM).

### C2: DAG-only topology

Pipelines are directed acyclic graphs. Cycles are rejected at config load time via topological sort (`petgraph`). Fan-out is supported via routers; fan-in is implicit (multiple upstream edges converge on a single node's `mpsc` receiver). There are no feedback loops, windowing operators, or cyclic stream primitives.

### C3: Bounded queues with backpressure

Every edge between nodes is a bounded `tokio::mpsc` channel (default capacity 1024). When a queue fills, the `OverflowPolicy` determines behaviour: `slow` (sender blocks — backpressure propagates upstream), `drop` (message discarded), or `dead-letter` (message routed to DLQ). Unbounded queues do not exist.

### C4: Wasm Component Model sandbox

Every processing stage (transform, filter, router) runs in its own `wasmtime::Store` with independent linear memory, fuel metering, epoch interruption, and WASI capability scoping. Stages cannot access each other's memory. The only communication path is through the host-mediated bounded queues.

### C5: Edge hardware target

The primary deployment target is Linux-capable devices with ≥ 4 GB RAM (Raspberry Pi 4, Jetson Orin Nano). The architecture does not assume cloud-scale resources, Kubernetes, or more than a single machine. This constraint drives decisions around single-process design, AOT compilation caching, memory limits, and the absence of distributed coordination.

### C6: Stateless node replacement

Hot-swap replaces a node's Wasm instance between messages. It does not preserve guest-side state across versions. This is a deliberate scope limitation matching edge workloads where pipeline stages are stateless transforms. Stateful operators (windowing, aggregation) require the plugin to persist state externally.

### C7: WIT as the contract boundary

All plugin-to-host interaction is defined in WIT (WebAssembly Interface Types). The four packages (`pipeline:types`, `pipeline:node`, `pipeline:routing`, `pipeline:host`, all `@0.1.0`) form the contract. Any language that compiles to `wasm32-wasip2` and implements the appropriate world can serve as a pipeline stage.

## Scope qualifiers

These terms have bounded meanings throughout the architecture:

- **"Edge gateway"** = Linux-capable devices with ≥ 4 GB RAM. Not microcontrollers.
- **"Isolation"** = memory containment + capability scoping. Not information-flow control or covert-channel elimination.
- **"Hot-swap"** = stateless node replacement. Not state-preserving live update.
- **"Competitive performance"** = within 30 % of eKuiper for the same workload. Not near-native for arbitrary computation.
- **"Pipeline"** = stateless transform DAG (parse, filter, route, inference). Not a full stream processor with windowing or exactly-once semantics.

## Related documents

- [RFC-001 WIT Contracts](../rfcs/RFC-001-wit-contracts.md) — contract design rationale.
- [RFC-002 Host Runtime](../rfcs/RFC-002-host-runtime.md) — engine and sandbox decisions.
- [RFC-005 Orchestrator](../rfcs/RFC-005-orchestrator.md) — hot-swap mechanism design.
- [ADR-0002 Bounded Queues](../adr/0002-spsc-bounded-queues.md) — queue topology choice.
- [ADR-0008 Error Policy Engine](../adr/0008-error-policy-engine.md) — five-category dispatch.
