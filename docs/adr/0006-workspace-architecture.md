# ADR-0006: Workspace Architecture for Runtime Control Plane

- **Date**: 2026-02-28
- **Status**: Accepted
- **SPEC Reference**: Section 12 (Observability), Section 9 (Dynamic Topology)

> **Current amendment.** The workspace-with-crates decision stands, but the
> shape below is the 2026-02-28 layout. The root `Cargo.toml` now has seven
> members: `wafer-types`, `wafer-config` (TOML loading, semantic validation,
> standalone DAG construction), `wafer-core`, `wafer-runtime`, `wafer-plugin`
> (guest SDK), `wafer-loadgen` (library and binary for evaluation traffic),
> and `waferctl`. Plugins under `plugins/` are excluded from the workspace.
> [`docs/learn/workspace-map.md`](../learn/workspace-map.md) is the current
> map of crate boundaries.
>
> The orchestrator design in "Internal Mutability for Arc Sharing" no longer
> exists: there is no `DagOrchestrator`, `PipelineControl` trait, or
> `Mutex<Option<RunState>>`. `PipelineOrchestrator`
> (`crates/wafer-core/src/orchestrator/pipeline.rs`) spawns one task per node
> into a `JoinSet`, and the API server holds a cloneable `PipelineHandle` that
> reads per-node state and metrics through atomics and signals hot-swap over
> per-node watch channels. See [ADR-0003](0003-hot-swap-mechanism.md),
> [ADR-0012](0012-watch-channel-hot-swap.md), and
> [RFC-005](../rfcs/RFC-005-orchestrator.md) for that task-per-node model. The
> commands now run through mise ([ADR-0015](0015-command-runner-mise.md)), not
> a justfile.

## Context

The WAFER runtime started as a single crate (`wafer`) containing all functionality. As we implemented the control plane features (HTTP API, CLI management tool, shared types), it became clear that the monolithic structure had limitations:

1. **Feature coupling**: Enabling HTTP API features pulled in dependencies (axum, tower-http) even for library users who didn't need them
2. **Binary separation**: The runtime binary and CLI tool have different concerns but shared types
3. **Build time**: Changes to CLI code recompiled core runtime unnecessarily
4. **Testing isolation**: Tests for different components were interleaved

### Requirements

- HTTP API server for runtime management (health, metrics, node control)
- CLI tool (`waferctl`) for operator interaction
- Shared type definitions between API server and CLI
- Core runtime usable as library without HTTP dependencies
- Clean separation of concerns

### Alternatives Considered

| Approach | Pros | Cons |
|----------|------|------|
| Single crate with features | Simpler structure | Feature flags complex, still coupled |
| Workspace with crates | Clean separation, independent versioning | More files, cross-crate dependencies |
| Separate repositories | Maximum isolation | Coordination overhead, version sync pain |

## Decision

Adopt a **Cargo workspace** with four crates:

```
crates/
├── wafer-core/        # Core runtime library
├── wafer-types/       # Shared API types (no dependencies)
├── wafer-runtime/     # Runtime binary (thin wrapper)
└── waferctl/          # CLI management tool
```

### Crate Responsibilities

**wafer-core**
- All runtime logic: engine, DAG orchestration, nodes, queues
- `PipelineControl` trait for management interface
- HTTP API server (feature-gated behind `http-api`)
- Heavy dependencies: wasmtime, tokio, petgraph, rumqttc

**wafer-types**
- Pure data types: `PipelineStatus`, `NodeInfo`, `ControlError`
- Request/response DTOs for API
- Minimal dependencies: serde only
- Used by both wafer-core (server) and waferctl (client)

**wafer-runtime**
- Thin binary wrapper around wafer-core
- CLI argument parsing (clap)
- Signal handling (SIGTERM, SIGINT)
- Tracing initialization
- Imports wafer-core with `http-api` feature enabled

**waferctl**
- Standalone CLI tool for operators
- HTTP client to talk to wafer-runtime
- Endpoint configuration management
- Human-readable and JSON output
- Uses wafer-types for deserializing API responses

### Dependency Graph

```
wafer-types (leaf - no internal deps)
     ↑
     ├── wafer-core (core lib)
     │        ↑
     │        └── wafer-runtime (binary)
     │
     └── waferctl (binary, no wafer-core dep)
```

Key insight: `waferctl` does NOT depend on `wafer-core`. It only shares types via `wafer-types`. This keeps the CLI lightweight and fast to compile.

### Internal Mutability for Arc Sharing

A key architectural decision within wafer-core: the `DagOrchestrator::run()` method was changed from `&mut self` to `&self` using internal mutability (`Mutex<Option<RunState>>`). This enables:

```rust
// API server and runner can share same orchestrator
let orchestrator = Arc::new(DagOrchestrator::from_config(config).await?);

// API server holds Arc clone
tokio::spawn(api_server::run(Arc::clone(&orchestrator), bind));

// Main thread runs pipeline - no &mut needed
orchestrator.run().await?;
```

This was Option 1 from the design exploration. Option 2 (separate handle pattern) was more complex with questionable benefits.

## Consequences

### Positive

1. **Clean dependency boundaries**: waferctl builds in ~2 seconds, wafer-core in ~30 seconds
2. **Feature isolation**: Library users can use wafer-core without HTTP dependencies
3. **Type sharing without coupling**: wafer-types is the only shared code between server and client
4. **Independent testing**: Each crate has focused tests
5. **Future flexibility**: Crates can be versioned and published independently

### Negative

1. **More files**: 4 Cargo.toml files, 4 lib.rs/main.rs files
2. **Import changes**: `use wafer_core::...` instead of `use wafer::...`
3. **Workspace coordination**: Must remember `--workspace` for commands

### Neutral

1. **Justfile updated**: `just build`, `just test` now use `--workspace`
2. **Tests reorganized**: 108 in wafer-core, 17 in wafer-types, 9 in waferctl

## Implementation Notes

The restructure was done in runtime-control-plane change tasks 1.1-1.10:

1. Created workspace Cargo.toml at root
2. Moved existing src/ to crates/wafer-core/src/
3. Created new crates with appropriate dependencies
4. Updated all import paths
5. Added workspace commands to justfile

All 124 tests pass across the workspace.
