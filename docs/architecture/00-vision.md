# Vision

WAFER (**W**eb**A**ssembly **F**low **E**xecution **R**untime) is a single-process pipeline runtime for IoT edge gateways, built on one idea: the stage is the unit of failure and update. The host owns everything that must outlive a stage, namely the pipeline graph, the bounded queue in front of each stage, and the connections to the outside. Each processing stage is a WebAssembly component in its own sandbox inside the host process. Filesystem access is never granted; stdio, environment, inference, and exact-destination outbound HTTP are granted per stage. Inference is restricted to Wasm Transforms, while outbound HTTP is available to Wasm Transform, Filter, and Router nodes.

## Problem

Edge gateways sit between constrained sensors and cloud analytics. They parse, filter, route, and enrich telemetry continuously, and their stages often come from different vendors and change after deployment. A fault in one stage must not stop the gateway, and an operator must be able to change one stage while the rest keeps running. Existing approaches force a trade-off:

- **Cloud-managed platforms** (Azure IoT Operations) use the same Wasm+WIT DAG model but require Kubernetes and ≥16 GB RAM, which rules out sub-8 GB gateways.
- **Monolithic edge processors** (eKuiper) run pipeline logic in a single address space with no per-stage isolation and no way to replace one operator while the rest keeps running.

WAFER targets the gap: typed Wasm stages, per-stage fault containment, and hot swap of one stage at a time, on a Raspberry Pi 5 4 GB running a single Linux process.

## Contribution

The contribution is the integrated runtime, not any single feature:

1. **Typed stages** — one `wafer:pipeline@0.1.0` package with Transform, Filter, Router, and capability-gated Inference worlds defines the host/guest contract. The host checks every component against its role before it runs, at startup and on every hot swap.
2. **Fault containment at stage granularity** — separate Wasm stores, memory limits, fuel metering, epoch interruption, and capability scoping keep a misbehaving stage from affecting its neighbours. A stage that traps is rebuilt alone from its compiled component.
3. **Hot swap** — a new version is prepared off the processing path and adopted between two records. The stage's queue, limits, and permissions stay in place, so a stage that keeps no state between records is replaced without losing or duplicating messages. Loss and duplication are verified from sink and sequence evidence, not from the swap response.

## What the evaluation asks

| Question | One-line summary |
|----|-----------------|
| **Performance** | Quantify the Wasm boundary overhead against native Rust and eKuiper on constrained hardware. |
| **Fault containment** | Show that per-stage sandboxes contain faults (memory, CPU, capabilities) without pipeline-wide failure. |
| **Hot swap** | Measure the disruption of replacing a stage at runtime: pause duration, message accounting, throughput dip. |

## Hardware Targets

- **Primary:** Raspberry Pi 5 (4 GB RAM, Cortex-A76, aarch64).
- **Replication:** Jetson Orin Nano (CPU only) and an x86-64 Linux host run the same experiments as the Pi. The earlier CUDA-provider MNIST run on Jetson is a separate diagnostic, and its teardown stability remains unresolved.
- **Development:** Apple Silicon / x86-64 Linux (any machine that runs `wasm32-wasip2` guests).

## Key Decisions

- [ADR-0001 — Wasmtime as the Wasm runtime](../adr/0001-wasmtime-runtime.md)
- [ADR-0006 — Workspace architecture (7-crate layout)](../adr/0006-workspace-architecture.md)
- [ADR-0016 — Default-deny outbound `wasi:http`](../adr/0016-outbound-wasi-http-capability.md)

## Next

For detailed quality attributes, constraints, and non-goals see [01-goals-and-constraints.md](./01-goals-and-constraints.md).
