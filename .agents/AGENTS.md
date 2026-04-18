# Agent Instructions

## Project Overview

**WAFER** (WebAssembly Flow Execution Runtime) is a high-performance, Rust-based DAG pipeline runtime that executes WebAssembly plugins using [Wasmtime](https://wasmtime.dev/). It is designed for building data processing pipelines with hot-swappable transforms, bounded queues with backpressure, and flexible fan-out/fan-in topologies.

### Key Concepts

- **DAG pipelines** — Data flows are defined as directed acyclic graphs via TOML configuration files
- **WebAssembly plugins** — Transforms, routers, and joiners are WASM components with WIT-defined interfaces
- **Bounded queues** — SPSC queues with configurable capacity and overflow policies (`slow`/backpressure, `drop`, `dead-letter`)
- **Hot-swap** — WASM plugins can be replaced at runtime without stopping the pipeline (drain-and-flip mechanism)
- **Fan-out/Fan-in** — Router nodes (1→N) and Joiner nodes (N→1) enable complex topologies like diamond patterns and scatter-gather
- **Multiple I/O** — Sources and sinks support stdin/stdout, files, MQTT pub/sub, and HTTP webhooks
- **Control plane** — HTTP REST API + Prometheus metrics for monitoring and management
- **OCI registry** — Load plugins from container registries (ghcr.io, Docker Hub) with local caching
- **Fuel-based metering** — Execution limits for untrusted plugins via wasmtime fuel

### Tech Stack

- **Rust 1.93+** (toolchain pinned in `rust-toolchain.toml`)
- **wasmtime** — WebAssembly runtime with WASI Preview 2 / Component Model
- **wit-bindgen** — Code generation from WIT interface definitions
- **petgraph** — DAG topology management
- **tokio** — Async runtime
- **axum** — HTTP control plane server

### Workspace Crates

| Crate | Type | Description |
|-------|------|-------------|
| `wafer-core` | Library | Core library: DAG orchestrator, engine, node implementations, queues, hot-swap, metrics |
| `wafer-runtime` | Binary | CLI runtime that loads TOML configs and runs pipelines, integrates the API server |
| `wafer-types` | Library | Shared types: error definitions, control messages, metric types |
| `waferctl` | Binary | CLI tool for interacting with running pipelines (status, nodes, drain, hot-swap) |

### Key Directories

| Directory | Contents |
|-----------|----------|
| `crates/` | Rust workspace crates (see table above) |
| `plugins/` | WebAssembly plugin source code (pass-through, uppercase, json-parse, filter, content-router, merge-joiner, mnist-inference) |
| `wit/` | WIT interface definitions for plugin worlds (`transform-node`, `router-node`, `joiner-node`) |
| `examples/` | Example pipeline TOML configurations (passthrough, uppercase, filter, chain, diamond, file-io, MQTT, etc.) |
| `tests/` | Integration tests |
| `docs/` | Project documentation (see Documentation Map below) |
| `specs/` | Feature specifications (OpenSpec workflow) |
| `scripts/` | Helper scripts |
| `models/` | ML model files (e.g., MNIST ONNX model for inference plugin) |

---

## Documentation Map

When you need deeper context on any aspect of the project, consult these files. Each entry includes a summary so you know what to expect before reading.

### Design & Specification

| Document | Summary |
|----------|---------|
| `docs/SPEC.md` | **The authoritative design reference.** Full technical specification covering architecture, WIT contracts, node categories, data types, pipeline configuration, queue/backpressure design, dynamic topology, hot-swap mechanism, inference capability, observability, security model, failure modes, and milestones. Read this first when making architectural decisions. |
| `docs/MVP.md` | **Current implementation status** (v0.4.0). Documents what's built, what's in progress, and what's next. Check this to understand the gap between the spec and reality — not everything in SPEC.md is implemented yet. |
| `docs/adr/` | **Architecture Decision Records.** 6 accepted ADRs: (1) wasmtime runtime selection, (2) SPSC bounded queues, (3) drain-and-flip hot-swap, (4) native sources/sinks, (5) OCI registry support, (6) workspace architecture. See `docs/adr/README.md` for the template, index, and conventions. |
| `specs/` | **Feature specifications directory.** Contains detailed specs for planned features using the OpenSpec workflow. See `specs/README.md` for structure and how to create new specs. |
| `todo.md` | **Known tech debt and refactor items.** Short list — check before starting refactoring work to see if it's already tracked. |

### API & Integration

| Document | Summary |
|----------|---------|
| `docs/api.md` | **HTTP control plane API reference.** Documents all endpoints (health, ready, pipeline status, node info, hot-swap, drain/shutdown) with request/response examples and implementation status. |
| `docs/api/openapi.yaml` | **OpenAPI 3.0 spec** for the control plane. Machine-readable API definition — useful for generating clients or validating responses. |
| `docs/api/bruno-collection/` | **Bruno HTTP client collection** for interactive API testing. Import into [Bruno](https://usebruno.com/) to explore the API. |
| `docs/REGISTRY.md` | **OCI registry integration guide.** How to publish WASM plugins to ghcr.io/Docker Hub, pull them in pipeline configs via `plugin_ref`, configure caching, and use `wkg` CLI tooling. |
| `docs/mqtt-setup.md` | **Local MQTT broker setup.** Docker-based Mosquitto configuration for developing and testing MQTT sources and sinks. Includes `mosquitto.conf` reference. |

### Benchmarks & Workflow

| Document | Summary |
|----------|---------|
| `docs/benchmarks/hot-swap.md` | **Hot-swap benchmark results.** Measures the prepare phase (load + instantiate new WASM component): ~8.85ms average, well under the SPEC §10.1 target of <50ms. Includes p50/p95/p99 percentiles. |
| `docs/AI_WORKFLOW.md` | **AI-assisted development workflow** (human-facing). Describes the tasks task tracking system and agent orchestration approach used in this project. |

### Root Files

| File | Summary |
|------|---------|
| `README.md` | **Project README.** Quick start, installation prerequisites, project structure, development setup, running pipelines, building plugins, control plane API overview, full configuration reference (node types, overflow policies, env vars), and GPU/CUDA setup for Jetson. The most comprehensive single-file overview. |
| `justfile` | **Command runner recipes.** All `just` commands for building, testing, running, plugin management, registry operations, and more. Run `just` with no args to see the full list. |
| `rust-toolchain.toml` | **Pinned Rust toolchain.** Ensures consistent Rust version (1.93) and targets (`wasm32-wasip2`) across all contributors. |
| `rustfmt.toml` | **Formatter configuration.** Rust formatting rules for the project. |
| `Cargo.toml` | **Workspace root.** Defines workspace members, shared dependencies, and profiles. |

---

## Build & Test Commands

This project uses [`just`](https://github.com/casey/just) as the command runner. All recipes are defined in the `justfile` at the project root.

### Core Workflow

```bash
just                    # List all available commands
just build              # Build entire workspace
just test               # Run all tests
just check              # Type-check without building
just fmt                # Format code with rustfmt
just clippy             # Run clippy lints (treats warnings as errors)
```

### Building Components

```bash
just build-runtime      # Build wafer-runtime only
just build-ctl          # Build waferctl only
just build-plugins      # Build all WASM plugins
just build-plugin NAME  # Build a specific plugin (e.g., just build-plugin uppercase)
```

### Running Pipelines

```bash
just run                                    # Run default passthrough pipeline
just run examples/dag-uppercase.toml        # Run a specific pipeline config
just run-remote                             # Run with OCI-hosted plugins
```

### Testing

```bash
cargo test --workspace                          # All tests
cargo test -p wafer-core                        # Single crate
cargo test -p wafer-core --features http-api    # With feature flags
cargo test -p wafer-runtime --test integration  # Integration tests only
cargo test --workspace -- --nocapture           # With stdout output
```

### Debugging

```bash
RUST_LOG=debug cargo run -p wafer-runtime -- --config examples/dag-passthrough.toml
RUST_LOG=trace cargo run -p wafer-runtime -- --config examples/dag-passthrough.toml
```

---

## Coding Conventions

For detailed Rust idioms, patterns, and style guidance, load the relevant skill for your task:

| Skill | When to Load |
|-------|-------------|
| `rust-best-practices` | Writing or reviewing any Rust code — covers idiomatic patterns, borrowing vs cloning, error handling, testing, documentation, and clippy usage |
| `cargo-expert` | Managing dependencies, workspace configuration, build targets, profiles, or troubleshooting build issues |
| `wasm-specialist` | Working with wasmtime, WASM plugin architecture, WIT interface definitions, WASI capabilities, or Component Model design |

### Key Rules

- **Formatting**: All code must pass `rustfmt` (configuration in `rustfmt.toml`)
- **Linting**: All code must pass `clippy -D warnings` — warnings are treated as errors
- **Toolchain**: Pinned in `rust-toolchain.toml` — do not change without discussion
- **Tests**: Add tests for new functionality. Run `just test` before considering work complete
- **WASM target**: Plugins compile to `wasm32-wasip2`. The toolchain file configures this target automatically

---

## ADR Workflow

When an architectural decision is needed:

1. **Research** options and document trade-offs
2. **Write** a proposed ADR in `docs/adr/NNNN-<slug>.md` with Status: **Proposed**
3. **Follow** the template in `docs/adr/README.md`
4. **Cross-reference** the relevant SPEC.md section if applicable
5. **Present** to the user for review — user accepts or rejects
6. **On acceptance**, update status to **Accepted** and create implementation tasks

Existing ADRs:
- `0001` — Wasmtime runtime selection
- `0002` — SPSC bounded queues
- `0003` — Drain-and-flip hot-swap
- `0004` — Native sources and sinks
- `0005` — Registry/package support
- `0006` — Workspace architecture
