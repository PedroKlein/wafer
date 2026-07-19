# Agent Instructions

## Thesis Context

This repo is the **experimental artifact** for an undergraduate thesis (TCC, UFRGS). The research design, evaluation methodology, and thesis framing live in a sibling repo.

| What you need | Where to find it |
|---------------|------------------|
| **What to build next** | `TODO.md` and `ROADMAP.md` (repo root) — implementation tasks and cross-references |
| **Which document is authoritative** | `tcc-doc/SOURCES-OF-TRUTH.md` |
| **How experiments should run** | `tcc-doc/research/analysis/evaluation-plan.md` |
| **RQs and pass/fail criteria** | `tcc-doc/research/analysis/thesis-statement-v3.md` |
| **Pipeline topologies to implement** | `tcc-doc/context/use-cases.md` |
| **Old RQ4/5/6 references** | `tcc-doc/RQ-VERSION-MAP.md` (they map to current RQ1–3) |
| **Literature on a topic** | Obsidian vault `TCC/papers/` |

**Sibling repos** (pi-repos group `tcc`):
- `github.com/PedroKlein/tcc-doc` — research, evaluation plan, thesis writing
- `github.com/PedroKlein/obsidian-personal` — knowledge base (notes under `TCC/`)

**Key framing:** The runtime IS the contribution (not just hot-swap). RQ1=Performance, RQ2=Isolation, RQ3=Hot-swap.

---

## Project Overview

**WAFER** (WebAssembly Flow Execution Runtime) is a single-process Rust runtime that executes typed DAGs of WebAssembly components on edge gateways. It uses [Wasmtime](https://wasmtime.dev/) with the Component Model. Pipelines are declared in TOML, connected by bounded `mpsc` queues with backpressure, and individual Wasm nodes can be hot-swapped between messages without pipeline downtime.

### Key Concepts

- **DAG pipelines** — Data flows are declared as directed acyclic graphs via TOML configuration files. Cycles are rejected at build time.
- **Five node categories** — `Source`, `Sink`, `Transform`, `Filter`, `Router`. Sources and Sinks are native Rust; Transform / Filter / Router are Wasm components.
- **WIT-typed boundaries** — Every Wasm call goes through one of the four `pipeline:*@0.1.0` WIT packages (`pipeline:types`, `pipeline:node`, `pipeline:routing`, `pipeline:host`). Worlds: `transform-node`, `filter-node`, `inference-node`, `router-node`.
- **Bounded queues** — Every edge is a bounded tokio `mpsc` channel with a configurable capacity and an overflow policy (`slow`/backpressure, `drop`, `dead-letter`). Backpressure propagates end-to-end.
- **Fan-out / fan-in** — Fan-out is expressed by Router nodes (1→N) that return output port names. Fan-in (N→1) is an **implicit host topology**: multiple upstream nodes are wired as multi-producer senders on the downstream node's single `mpsc` receiver. There is no first-class Joiner node type; fan-in requires no Wasm.
- **Hot-swap (watch-channel, between messages)** — Each Wasm node's task holds a `watch::Receiver<Option<SwapPayload>>`. At the top of every runner loop iteration the task polls `swap_rx.has_changed()` — if a swap payload is available it drops the old instance and installs the pre-instantiated replacement, then enters its usual `select!` between cancellation and the input channel. State inside the guest instance is lost by design (see Invariant 7).
- **Zero-copy envelope** — Messages cross the boundary as an `Arc<EnvelopeHeader>` plus `Bytes` payload plus lineage trail. Guests read the payload via a `borrow<buffer>` resource handle exposed by `pipeline:types`.
- **Error policy** — Every host-observed guest error maps to one of five categories (`bad-input`, `dependency-failed`, `processing-failed`, `timed-out`, `unrecoverable`) which the policy engine dispatches to `retry` / `dead-letter` / `skip` / `teardown`.
- **Multiple I/O** — Native sources and sinks support stdin/stdout, files, MQTT pub/sub, and HTTP webhooks.
- **Control plane** — axum HTTP REST API + Prometheus metrics on a separate port.
- **OCI registry** — Wasm components can be pulled from container registries (ghcr.io, Docker Hub) via the same `plugin` field used for local paths, with content-addressable local caching.
- **Fuel + epoch metering** — Untrusted guests are bounded in both computation (fuel) and wall-clock time (epoch interruption on a dedicated OS thread).

### Tech Stack

- **Rust stable** (toolchain pinned in `rust-toolchain.toml`, Edition 2024)
- **wasmtime** — WebAssembly runtime with WASI Preview 2 / Component Model
- **wit-bindgen** — Code generation from WIT interface definitions
- **petgraph** — DAG topology management (toposort + cycle detection)
- **tokio** — Async runtime (`mpsc`, `watch`, `select!`)
- **axum** — HTTP control plane server
- **prometheus-client** — Metrics exposition

### Workspace Crates

| Crate | Type | Description |
|-------|------|-------------|
| `wafer-core` | Library | Core runtime: DAG orchestrator, engine, node runners, queue wiring, hot-swap, error policy, metrics, HTTP API surface |
| `wafer-runtime` | Binary | CLI runtime that loads TOML configs and runs pipelines; wires the API server into `wafer-core` |
| `wafer-types` | Library | Shared types: config schema (`NodeDef`, `WasmNodeDef`, `EdgeDef`, `EngineConfig`, `ErrorPolicyConfig`), control messages, event types, metrics types |
| `wafer-config` | Library | TOML loading, DAG construction, semantic validation on top of `wafer-types` |
| `wafer-plugin` | Library | Guest-side SDK (macros, `thread_local!` state pattern, error helpers) that plugin crates depend on |
| `wafer-loadgen` | Binary | Load generator used by the evaluation harness (open-loop, HdrHistogram) |
| `waferctl` | Binary | CLI tool for interacting with running pipelines (status, nodes, hot-swap, shutdown) |

### Key Directories

| Directory | Contents |
|-----------|----------|
| `crates/` | Rust workspace crates (see table above) |
| `plugins/` | Wasm plugin source (e.g. `pass-through`, `uppercase`, `json-parse`, `threshold-filter`, `content-router`, `mnist-inference`, `tensor-prep`, `cayenne-decoder`, `quality-rules`, `anomaly-detector`, `vibration-features`, `result-format`, `attacks/…`) |
| `wit/` | WIT interface definitions for the four `pipeline:*@0.1.0` packages and the `transform-node` / `filter-node` / `inference-node` / `router-node` worlds |
| `examples/` | Runtime-schema pipeline TOML examples (passthrough, uppercase, filter, chain, fanout, file-io, mqtt, http, overflow-dlq-demo, metrics-demo, mnist-inference, remote OCI, …). See `examples/README.md` before copying config shape. |
| `tests/` | Integration tests |
| `docs/` | Project documentation (see Documentation Map below) |
| `specs/` | Feature specifications (OpenSpec workflow) |
| `scripts/` | Helper scripts |
| `models/` | ML model files (e.g., MNIST ONNX model for inference plugin) |
| `eval/` | Evaluation harness inputs / outputs |

---

## Documentation Map

When you need deeper context on any aspect of the project, consult these files. The `docs/` tree follows an arc42-lite layout: architecture views, RFCs, ADRs, interfaces, operations guides, status reports, and workflows. Each entry includes a summary so you know what to expect before reading. If documentation and implementation disagree, trust the source code, WIT files under `wit/`, and `docs/status/implementation-gaps.md`.

### Design & Specification

| Document | Summary |
|----------|---------|
| `docs/architecture/` | arc42-lite architecture views: vision, goals & constraints, solution strategy, building blocks, runtime view, deployment, cross-cutting concepts, quality requirements, risks, comparators. |
| `docs/status/implementation-status.md` | Current implementation status — what's built, what's tested, per-plugin coverage. Replaces the old monolithic MVP status doc. |
| `docs/rfcs/` | RFC archive — long-form design decisions with Abstract, Alternatives Considered, Related RFCs, Implementation Notes. Ten RFCs cover WIT contracts, host runtime, node types, config schema, orchestrator, plugin SDK, performance, evaluation harness, implementation architecture, and I/O integration. |
| `docs/adr/` | Architecture Decision Records in Michael Nygard format (short, executive). See `docs/adr/README.md` for the index and conventions. |
| `specs/` | Feature specifications directory (OpenSpec workflow). See `specs/README.md`. |
| `TODO.md` | Tactical implementation task list. |
| `ROADMAP.md` | Aspirational / longer-horizon items flagged in RFCs and the evaluation plan. |

### API & Integration

| Document | Summary |
|----------|---------|
| `docs/interfaces/http-api.md` | HTTP control plane reference. Current endpoints: `/health`, `/ready`, `/metrics`, `/api/v1/nodes`, `/api/v1/nodes/{id}`, `/api/v1/nodes/{id}/hot-swap`, `/api/v1/pipeline/shutdown`. |
| `docs/interfaces/wit-contracts.md` | Reference for the four WIT packages and their worlds. |
| `docs/interfaces/config-schema.md` | TOML config reference. Nodes use a `[nodes.NAME]` map, each `WasmNodeDef` has a single `plugin` field (local path or OCI reference), and each `EdgeDef` has a single optional `port` field (only used for router outputs). |
| `docs/interfaces/plugin-sdk.md` | Reference for the `wafer-plugin` guest SDK (macros, `thread_local!` + `RefCell` state pattern, error helpers). |
| `docs/api/openapi.yaml` | OpenAPI 3.0 spec for the control plane (kept in sync with the axum handlers). |
| `docs/api/bruno-collection/` | Bruno HTTP client collection for interactive API testing. |
| `docs/operations/registry.md` | OCI registry integration guide — publishing Wasm components to ghcr.io/Docker Hub, pulling via the `plugin` field, configuring caching, and using `wkg` tooling. |
| `docs/operations/mqtt-setup.md` | Local MQTT broker setup (Docker Mosquitto) for developing and testing MQTT sources and sinks. |

### Benchmarks & Workflow

| Document | Summary |
|----------|---------|
| `docs/benchmarks/hot-swap.md` | Hot-swap benchmark reference with per-phase timing (compile, instantiate, signal, ack, convergence) for the watch-channel model. |
| `docs/AI_WORKFLOW.md` | AI-assisted development workflow (human-facing). Describes the task tracking system and agent orchestration approach used in this project. |

### Root Files

| File | Summary |
|------|---------|
| `README.md` | Project README — quick start, prerequisites, project structure, development setup, running pipelines, building plugins. Post-migration, this is a one-page quickstart that points into `docs/`. |
| `justfile` | Command runner recipes. Run `just` with no args to see the full list. |
| `rust-toolchain.toml` | Pinned Rust toolchain (stable channel, `wasm32-wasip2` target). |
| `rustfmt.toml` | Formatter configuration. |
| `Cargo.toml` | Workspace root. Defines workspace members, shared dependencies, and profiles. |

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
| `wasm-specialist` | Working with wasmtime, Wasm plugin architecture, WIT interface definitions, WASI capabilities, or Component Model design |
| `async-tokio` | Working with `mpsc`, `watch`, `select!`, cancellation, or the hot-swap coordination code |
| `dag-orchestration` | Working with pipeline topology, petgraph, or fan-in/fan-out wiring |

### Key Rules

- **Formatting**: All code must pass `rustfmt` (configuration in `rustfmt.toml`)
- **Linting**: All code must pass `clippy -D warnings` — warnings are treated as errors
- **Toolchain**: Pinned in `rust-toolchain.toml` — do not change without discussion
- **Tests**: Add tests for new functionality. Run `just test` before considering work complete
- **WASM target**: Plugins compile to `wasm32-wasip2`. The toolchain file configures this target automatically

---

## ADR & RFC Workflow

Two-tier decision archive:

- **RFCs** (`docs/rfcs/RFC-NNN-<slug>.md`) capture the long-form reasoning: abstract, decision, alternatives considered, related RFCs, implementation notes.
- **ADRs** (`docs/adr/NNNN-<slug>.md`) are short Nygard-format summaries (Context / Decision / Consequences / See Also) that link back to their parent RFC.

When an architectural decision is needed:

1. **Research** options and document trade-offs
2. **Draft** an RFC under `docs/rfcs/` (or a proposed ADR under `docs/adr/` for narrower decisions)
3. **Follow** the templates and index entries in `docs/rfcs/README.md` and `docs/adr/README.md`
4. **Present** to the user for review — user accepts or rejects
5. **On acceptance**, update status to **Accepted** / **Implemented** and create implementation tasks. When an RFC introduces a distinct decision worth surfacing separately, also add a Nygard ADR that links back.
