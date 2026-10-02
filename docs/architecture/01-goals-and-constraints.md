# Goals and Constraints

> arc42 §1–3: driving forces, quality goals, stakeholders, and hard architectural constraints.

## System purpose

WAFER (WebAssembly Flow Execution Runtime) is a single-process DAG pipeline runtime for edge IoT gateways. It composes pipeline stages as typed, sandboxed WebAssembly Component Model modules connected by bounded queues with backpressure.

The evaluated contribution is the integration of typed DAG composition, per-stage fault isolation, and stateless live stage replacement on Linux gateways with at least 4 GB RAM. Performance, isolation, and hot-swap are evaluated as separate claim families.

## Research questions

The Raspberry Pi 5 4 GB decides every verdict. A Jetson Orin Nano (CPU only) and an x86-64 Linux host repeat the same experiments as replication strata. [07. Quality requirements](07-quality-requirements.md) maps each question to its experiments, and `verdict_rules` in `eval/canonical-matrix.json` holds the thresholds.

- **RQ1:** What is the performance cost of typed Wasm boundaries on edge hardware?
- **RQ2:** Do per-stage sandboxes contain faults without pipeline-wide failure?
- **RQ3:** What is the disruption cost of replacing a stage at runtime?

## Stakeholders

| Stakeholder | Concern | Addressed by |
|---|---|---|
| **Thesis reader / evaluator** | Reproducible evidence for the three research questions | Evaluation harness, quantitative benchmarks, documented methodology |
| **Edge-gateway operator** | 24/7 uptime on constrained hardware with safe updates | Single-process deployment, hot-swap, bounded memory, DLQ for failures |
| **Plugin author** | Ship logic in any Component-Model language without knowing the host | WIT-typed contracts, `wafer-plugin` guest SDK, per-node capability scoping |

## Quality goals (ranked)

1. **Fault containment**: a misbehaving plugin must not crash the host or corrupt sibling nodes.
2. **Bounded resource consumption**: memory and CPU usage must remain predictable regardless of plugin behaviour.
3. **Competitive performance**: target-load latency is bounded against eKuiper, while capacity compares delivery ceilings bracketed by tested rates below the shared MQTT support limit.
4. **Operational evolvability**: individual stages must be replaceable at runtime without pipeline downtime.
5. **Simplicity of deployment**: a single binary, a single TOML file, no container orchestrator.

## Hard architectural constraints

These are non-negotiable boundaries. They are facts of the current system, not aspirations.

### C1: Single process

The entire pipeline: sources, Wasm stages, sinks, control plane: runs in one OS process. There is no inter-process communication, no sidecar, no container-per-node. This is a deliberate design choice that maximises efficiency on gateway-class hardware (4 GB RAM, quad-core ARM).

### C2: DAG-only topology

Pipelines are directed acyclic graphs. Cycles are rejected at config load time via topological sort (`petgraph`). Fan-out is supported via routers; fan-in is implicit (multiple upstream edges converge on a single node's `mpsc` receiver). There are no feedback loops, windowing operators, or cyclic stream primitives.

### C3: Bounded queues with backpressure

Every destination has one bounded `tokio::mpsc` receiver. Edge-shaped configuration chooses sender policy; the physical queue uses the maximum explicit incoming capacity, or default 1024 when none is explicit. `slow` waits, `drop` discards on full, and `dead-letter` attempts the configured DLQ. Fan-in has no fairness or cross-producer ordering guarantee. Unbounded and zero-capacity queues do not exist.

### C4: Wasm Component Model sandbox

Every processing stage (transform, filter, router) runs in its own `wasmtime::Store` with independent linear memory and WASI capability scoping. Fuel and epoch interruption are available but optional at runtime. Final evaluation configs enable both explicitly except for declared ablations and attack stimuli. Stages cannot access each other's memory. Per-store OOM containment covers linear memory and tables; host-side WASI resources a guest creates are not bounded in this build. Time isolation bounds Wasm CPU time via epochs and fuel; a guest blocked in a host import is not bounded. Pipeline messages travel through host-mediated bounded queues; a Wasm processing node may separately receive exact-destination outbound `wasi:http` authority through static configuration.

### C5: Edge hardware target

The primary deployment target is a Linux-capable device with at least 4 GB RAM (Raspberry Pi 5 4 GB). The architecture does not assume cloud-scale resources, Kubernetes, or more than a single machine. Jetson Orin Nano and x86-64 Linux hosts replicate the evaluation; the CUDA inference path on Jetson is a separate diagnostic.

### C6: Stateless node replacement

Hot-swap replaces a node's Wasm instance between messages. It does not preserve guest-side state across versions: the replacement starts from its own `init`, and the outgoing instance's `close` export is not called. This is a deliberate scope limitation matching edge workloads where pipeline stages are stateless transforms. Stateful operators (windowing, aggregation) require the plugin to persist state externally.

### C7: WIT as the contract boundary

All project plugin-to-host interaction is defined in one WIT package, `wafer:pipeline@0.1.0`, containing `types`, `lifecycle`, `transform`, `filter`, `router`, and `logging` interfaces plus four worlds. `inference-node` is a default-deny Transform specialization that also imports the pinned `wasi:nn` package. The verified non-Rust claim is bounded to the TinyGo uppercase component; Python remains a stub.

## Scope qualifiers

These terms have bounded meanings throughout the architecture:

- **"Edge gateway"** = Linux-capable devices with ≥ 4 GB RAM. Not microcontrollers.
- **"Isolation"** = memory containment + capability scoping. Not information-flow control or covert-channel elimination.
- **"Hot-swap"** = stateless node replacement. Not state-preserving live update.
- **"Competitive performance"** = target-load p95 within 2x eKuiper and a WAFER tested-grid delivery ceiling at least 0.70 of eKuiper's, with each ceiling bracketed by tested rates. Not near-native performance for arbitrary computation.
- **"Pipeline"** = stateless processing DAG (parse, filter, route, capability-gated inference, and optionally bounded outbound HTTP). Windowing and exactly-once semantics are outside the current release.

## Related documents

- [RFC-001 WIT Contracts](../rfcs/RFC-001-wit-contracts.md): contract design rationale.
- [RFC-002 Host Runtime](../rfcs/RFC-002-host-runtime.md): engine and sandbox decisions.
- [RFC-005 Orchestrator](../rfcs/RFC-005-orchestrator.md): hot-swap mechanism design.
- [ADR-0002 Bounded Queues](../adr/0002-spsc-bounded-queues.md): queue topology choice.
- [ADR-0008 Error Policy Engine](../adr/0008-error-policy-engine.md): five-category dispatch.
- [ADR-0016 Default-Deny Outbound `wasi:http`](../adr/0016-outbound-wasi-http-capability.md): exact-destination network authority.
