# WAFER: WebAssembly Flow Execution Runtime

> A typed DAG pipeline runtime using WebAssembly components

## Vision

WAFER (wasm-dag-runtime) is a general-purpose runtime for executing Directed Acyclic Graphs (DAGs) of WebAssembly components. The runtime is designed to be embeddable, portable, and extensible—suitable for edge IoT gateways, API/proxy filters, observability agents, and data transformation pipelines.

## Core Value Propositions

| Value | Description |
|-------|-------------|
| **Type-safe boundaries** | WIT-defined contracts between host and nodes; validated at DAG load time |
| **Minimal-copy data flow** | Efficient data passing through the DAG; true zero-copy is future work |
| **Strong isolation** | WASI capability grants + Wasm sandboxing + fuel/epoch limits |
| **Cross-architecture portability** | Same `.wasm` binary runs on ARM (Pi, Jetson, Mac M3) and x86 |
| **Per-node hot-swap** | Upgrade individual nodes without stopping the pipeline |
| **Dynamic topology** | Add/remove nodes and edges at runtime |

## Project Scope

### Primary Focus (Thesis TG2)
- **Domain:** Edge IoT gateways with MQTT telemetry
- **Secondary:** Local ML inference on Jetson via wasi-nn
- **Platforms:** Raspberry Pi 4, MacBook M3, x86 workstation, Jetson AGX Orin

### Goals (In Scope)

| ID | Goal | Priority |
|----|------|----------|
| G1 | Type-safe DAG execution via WIT contracts | Must |
| G2 | Minimal-copy data passing between nodes | Must |
| G3 | Bounded queues with backpressure | Must |
| G4 | Per-node hot-swap (drain-and-flip) | Must (**primary thesis target**) |
| G5 | Dynamic topology at runtime | Should (stretch goal) |
| G6 | WASI capability isolation | Must |
| G7 | wasi-nn compatible inference | Must |
| G8 | Cross-architecture portability | Must |

### Non-Goals (Out of Scope)

| ID | Non-Goal | Rationale |
|----|----------|-----------|
| NG1 | Exactly-once semantics | At-least-once sufficient |
| NG2 | Distributed checkpointing | Single-node focus |
| NG3 | Kubernetes operator/CRDs | Future work |
| NG4 | Event-time semantics | Processing-time only |

## Architecture Overview

```
┌─────────────────────────────────────────────────────────────────┐
│                        wasm-dag-runtime                         │
├─────────────────────────────────────────────────────────────────┤
│  ┌─────────────┐  ┌─────────────┐  ┌─────────────┐             │
│  │   Config    │  │  Topology   │  │   Metrics   │             │
│  │   Loader    │  │   Manager   │  │  Collector  │             │
│  └──────┬──────┘  └──────┬──────┘  └──────┬──────┘             │
│         └────────────────┼────────────────┘                     │
│                          │                                      │
│  ┌───────────────────────▼───────────────────────┐             │
│  │               DAG Orchestrator                 │             │
│  │  • Node lifecycle (spawn, drain, retire)      │             │
│  │  • Queue management (bounded, backpressure)   │             │
│  │  • Type validation at connection time         │             │
│  │  • Hot-swap coordination                      │             │
│  └───────────────────────┬───────────────────────┘             │
│                          │                                      │
│  ┌───────────────────────▼───────────────────────┐             │
│  │             Wasmtime Engine Pool               │             │
│  │  • Component instantiation                    │             │
│  │  • Fuel/epoch metering                        │             │
│  │  • WASI capability injection                  │             │
│  └───────────────────────┬───────────────────────┘             │
│                          │                                      │
│  ┌─────────┐  ┌─────────┐  ┌─────────┐  ┌─────────┐           │
│  │ Source  │──│Transform│──│ Router  │──│  Sink   │           │
│  │ (Rust)  │  │ (.wasm) │  │ (.wasm) │  │ (Rust)  │           │
│  └─────────┘  └─────────┘  └─────────┘  └─────────┘           │
└─────────────────────────────────────────────────────────────────┘
```

## Node Types

| Category | Inputs | Outputs | Implementation | Purpose |
|----------|--------|---------|----------------|---------|
| Source | 0 (external) | 1 | Native Rust | Ingest data (MQTT, file, stdin) |
| Transform | 1 | 1 | WASM component | Process/transform messages |
| Router | 1 | N | WASM component | Content-based routing |
| Joiner | N | 1 | WASM component | Merge multiple streams |
| Sink | 1 | 0 (external) | Native Rust | Output data (MQTT, file, stdout) |

## Current State (MVP v0.3.0)

See [docs/MVP.md](../docs/MVP.md) for full implementation status.

**Implemented:**
- Linear chain execution with DAG support
- Router (1→N) and Joiner (N→1) nodes
- WASI Preview 2 component loading
- Fuel/epoch metering
- SPSC bounded queues
- MQTT, file, stdin/stdout sources/sinks
- OCI registry support for remote plugins
- wasi-nn inference (MNIST demo)

**Not Yet Implemented:**
- Hot-swap (drain-and-flip)
- Dynamic topology (add/remove nodes at runtime)
- Prometheus metrics endpoint
- REST API for topology changes

## Key Documentation

| Document | Purpose |
|----------|---------|
| [SPEC.md](../docs/SPEC.md) | Full specification (authoritative) |
| [MVP.md](../docs/MVP.md) | Current implementation status |
| [AI_WORKFLOW.md](../docs/AI_WORKFLOW.md) | Development workflow with agents |
| [REGISTRY.md](../docs/REGISTRY.md) | OCI registry guide |
| [ADRs](../docs/adr/) | Architecture Decision Records |

## Technology Stack

| Layer | Technology | Rationale |
|-------|------------|-----------|
| Language | Rust | Best Wasm component support |
| Async runtime | Tokio | Mature ecosystem |
| Wasm engine | Wasmtime | Best WASI P2 and Component Model |
| MQTT | rumqttc | Rust-native, async |
| Serialization | serde | Multi-format support |
| Queues | tokio::sync::mpsc | Bounded, async |
| Registry | oci-client | OCI registry access |
