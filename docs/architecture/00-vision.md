# Vision

WAFER (**W**eb**A**ssembly **F**low **E**xecution **R**untime) is a single-process DAG pipeline runtime for edge IoT gateways. Untrusted processing stages are sandboxed WebAssembly components connected by bounded, policy-aware queues. Filesystem and network interfaces are not exposed to processing guests; stdio, environment, and inference imports are granted explicitly, with inference restricted to Wasm Transforms.

## Problem

Edge gateways sit between constrained sensors and cloud analytics. They must parse, filter, route, and enrich telemetry streams continuously. Existing approaches force a trade-off:

- **Cloud-managed platforms** (Azure IoT Operations) use the same Wasm+WIT DAG model but require Kubernetes and ≥16 GB RAM — unsuitable for sub-8 GB gateways.
- **Monolithic edge processors** (eKuiper) run pipeline logic in a single address space with no per-stage isolation and no live-update mechanism.

WAFER targets the gap: typed Wasm composition, per-stage fault containment, and live node replacement on a Raspberry Pi 5 4 GB running a single Linux process.

## Contribution

The runtime architecture *is* the contribution. No single feature is elevated; the novelty is the integrated system that simultaneously delivers:

1. **Typed DAG composition** — one `wafer:pipeline@0.1.0` package with Transform, Filter, Router, and capability-gated Inference worlds defines the host/guest contract across five configured node categories.
2. **Per-stage fault isolation** in one OS process — separate Wasm stores, fuel metering, epoch interruption, and capability scoping ensure a misbehaving node cannot corrupt neighbours.
3. **Live stage replacement** — the watch-channel mechanism replaces a loaded Wasm processing node between messages. Loss and disruption claims require separate sink/sequence evidence.

## Research Questions

| RQ | One-line summary |
|----|-----------------|
| **RQ1 — Performance** | Quantify the Wasm boundary overhead against native Rust (isolation tax) and eKuiper (competitive viability) on constrained hardware. |
| **RQ2 — Isolation** | Demonstrate that per-stage sandboxes contain faults (memory, CPU, capabilities) without pipeline-wide failure. |
| **RQ3 — Hot-swap** | Measure disruption cost of replacing a stage at runtime: pause duration, message accounting, throughput dip. |

## Hardware Targets

- **Primary:** Raspberry Pi 5 (4 GB RAM, Cortex-A76, aarch64).
- **Secondary validation:** NVIDIA Jetson CPU and CUDA-provider execution of the MNIST path is verified on the restoration candidate. The run is diagnostic, not part of the final performance campaign, and CUDA teardown stability remains unresolved.
- **Development:** Apple Silicon / x86-64 Linux (any machine that runs `wasm32-wasip2` guests).

## Key Decisions

- [ADR-0001 — Wasmtime as the Wasm runtime](../adr/0001-wasmtime-runtime.md)
- [ADR-0006 — Workspace architecture (7-crate layout)](../adr/0006-workspace-architecture.md)

## Next

For detailed quality attributes, constraints, and non-goals see [01-goals-and-constraints.md](./01-goals-and-constraints.md).
