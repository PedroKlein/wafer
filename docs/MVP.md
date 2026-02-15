# WAFER MVP Documentation

> Current state of the WebAssembly Flow Execution Runtime proof-of-concept

**Version:** 0.2.0  
**Tech Stack:** Rust 1.88+, wasmtime 41.0.3, wit-bindgen 0.53.1, WASI Preview 2, petgraph

---

## Current State

The MVP implements **multi-node DAG pipelines** with:
- Linear chain execution: `FileSource → Transform → Transform → FileSink`
- WASI Preview 2 component loading via wasmtime
- WIT-based type contracts (`transform-node` world)
- Fuel-based execution metering with per-call reset
- Epoch interruption for cooperative scheduling
- Capability-scoped WASI contexts
- Generic I/O for testability
- **SPSC bounded queues integrated for inter-node communication**
- **petgraph-based DAG topology management**
- TOML configuration loading for both single-transform and DAG pipelines

**What works:**
- Single-transform pipeline: Load a transform plugin, pipe stdin through `process()`, write to stdout
- Multi-node DAG: Load DAG config, wire nodes with bounded queues, execute with backpressure, graceful shutdown

---

## Module Structure

```
src/
├── engine/
│   ├── host.rs       # WaferState: WasiView impl, Capabilities for scoping
│   ├── instance.rs   # TransformInstance: bindgen!, component instantiation
│   ├── loader.rs     # WaferEngine: wasmtime config, cached Linker, component loading
│   └── mod.rs
├── node/
│   ├── traits.rs     # Lifecycle and Transform traits
│   ├── transform.rs  # WasmTransform: trait impl wrapping TransformInstance
│   ├── source.rs     # Source trait + FileSource implementation
│   ├── sink.rs       # Sink trait + FileSink implementation
│   └── mod.rs        # AnyNode enum (Transform, Source, Sink variants)
├── dag/
│   ├── orchestrator.rs # DagOrchestrator: petgraph topology, queue wiring, async execution
│   └── mod.rs
├── pipeline/
│   ├── executor.rs   # PipelineExecutor<R, W>: generic I/O processing loop
│   ├── builder.rs    # PipelineBuilder: config→PipelineExecutorCore
│   └── mod.rs
├── queue/
│   ├── bounded.rs    # BoundedQueue<T>: SPSC with crossbeam-channel
│   ├── envelope.rs   # RuntimeEnvelope: host-side message wrapper
│   └── mod.rs
├── config/
│   ├── loader.rs     # TOML file loading
│   ├── schema.rs     # PipelineConfig, TransformConfig, DagConfig structs
│   └── mod.rs
├── metrics/
│   └── counters.rs   # PipelineMetrics: atomic counters, ProcessTimer
├── error.rs          # WaferError enum, ConfigError, Result type
├── lib.rs
└── main.rs           # CLI: --config flag, pipeline.toml
```

### WIT Files

```
wit/
├── types.wit         # Shared types: envelope, payload, process-result, metadata
├── lifecycle.wit     # Lifecycle interface: validate, init, close
└── transform.wit     # Transform interface and transform-node world
```

### External Files

| File | Purpose |
|------|---------|
| `wit/*.wit` | WIT contracts (flat single-package structure) |
| `plugins/pass-through/` | Example transform (no-op, emits input unchanged) |

---

## What's Implemented

### Core Runtime
- [x] Wasmtime engine with component model support
- [x] WASI Preview 2 via `wasmtime-wasi` P2 bindings
- [x] Async support (`async_support(true)`)
- [x] Fuel metering with per-call reset (default: 1,000,000)
- [x] Epoch interruption for cooperative scheduling
- [x] Cached Linker in `WaferEngine` via `OnceLock`
- [x] Component loading from `.wasm` files

### WIT Contract (`transform-node` world)
- [x] `types` interface: `Envelope`, `Payload`, `ProcessResult`, `ProcessError`, `Metadata`
- [x] `lifecycle` interface: `validate(NodeConfig)`, `init(NodeConfig)`, `close()`
- [x] `transform` interface: `process(Envelope) -> ProcessResult`
- [x] Payload variant: `raw(list<u8>)` only
- [x] Metadata as `list<tuple<string, string>>` (no external deps)

### Rust Trait Architecture
- [x] `Lifecycle` trait: `init`, `validate`, `close` async methods
- [x] `Transform` trait: `process` async method
- [x] `Source` trait: `poll` async method for data ingestion
- [x] `Sink` trait: `collect` async method for data output
- [x] `WasmTransform` struct implementing Lifecycle + Transform
- [x] `FileSource` struct implementing Lifecycle + Source (line-based file reading)
- [x] `FileSink` struct implementing Lifecycle + Sink (line-based file writing)
- [x] `AnyNode` enum with `Transform`, `Source`, `Sink` variants
- [x] Traits are `Send` but not `Sync` (WASM stores aren't thread-safe)

### Pipeline Execution
- [x] Single transform node execution (stdin/stdout)
- [x] Generic I/O: `PipelineExecutor<R: BufRead, W: Write>`
- [x] `PipelineExecutorCore` builder for deferred I/O attachment
- [x] `.with_io(reader, writer)` for testing
- [x] `.with_stdio()` for production
- [x] ProcessResult handling: `emit`, `filter`, `error`
- [x] Configuration via TOML files

### DAG Orchestration
- [x] `DagOrchestrator` with petgraph-based topology
- [x] `DagConfig` schema with nodes and edges definitions
- [x] Linear chain execution: `Source → Transform(s) → Sink`
- [x] Topological sort for execution order
- [x] Cycle detection (rejects invalid DAGs)
- [x] Bounded queue wiring between nodes
- [x] Async execution with tokio::spawn per node
- [x] Graceful shutdown in reverse topological order
- [x] Error handling: log and continue (non-fatal)

### Capability Scoping
- [x] `Capabilities` struct for security boundaries
- [x] `Capabilities::sandbox()` - minimal access
- [x] `Capabilities::with_stdio()` - inherit stdin/stdout (default)
- [x] `Capabilities::full()` - trusted plugins
- [x] Placeholders for network/filesystem scoping

### Infrastructure
- [x] Async bounded queues using `tokio::sync::mpsc` for inter-node communication
- [x] RuntimeEnvelope ↔ WIT Envelope conversion
- [x] Atomic metrics: `messages_total`, `process_time_ns`, `queue_depth`
- [x] ProcessTimer RAII guard for timing
- [x] Error types: `WaferError`, `ConfigError` (with `Message` variant)
- [x] Epoch ticker for cooperative WASM scheduling

### Example Plugin
- [x] `pass-through` transform (wit-bindgen 0.53.1)
- [x] Implements full lifecycle: `validate`, `init`, `close`
- [x] Compiles to `wasm32-wasip2` target

---

## What's NOT Implemented

### Node Types (SPEC §5)
- [x] ~~Source nodes (`poll`, `ack`)~~ - FileSource implemented (host-only, no WASM)
- [x] ~~Sink nodes (`collect`, `flush`)~~ - FileSink implemented (host-only, no WASM)
- [ ] Router nodes (1→N routing)
- [ ] Joiner nodes (N→1 merge)
- [ ] WASM-based Source/Sink (currently host-only Rust implementations)

### DAG Orchestration
- [x] ~~Multi-node pipelines~~ - Linear chain implemented
- [x] ~~Edge wiring between nodes~~ - BoundedQueue integration complete
- [x] ~~Queue integration for inter-node communication~~ - Done
- [x] ~~Backpressure propagation~~ - Blocking queues implemented
- [ ] Fan-out/fan-in topologies (Router/Joiner)
- [ ] CLI `--dag-config` flag (tests use DagOrchestrator directly)

### Dynamic Features
- [ ] Hot-swap (drain-and-flip)
- [ ] Dynamic topology (add/remove nodes)
- [ ] Config file watching
- [ ] REST API for topology changes

### External Integration
- [ ] MQTT source/sink
- [ ] Other external connectors

### Advanced WIT Types
- [ ] `json-value` payload variant
- [ ] `sensor-reading` payload variant
- [ ] `tensor` payload variant (wasi-nn)
- [ ] `image-data` payload variant

### Capabilities
- [ ] wasi-nn inference
- [ ] Host-managed node state
- [ ] Network capability enforcement (currently placeholder)
- [ ] Filesystem capability enforcement (currently placeholder)

---

## Known Limitations

| Area | Limitation |
|------|------------|
| **Pipeline** | Linear chains only (no fan-out/fan-in) |
| **I/O** | stdin/stdout for single-transform; file-based for DAG |
| **Source/Sink** | Host-only Rust implementations (no WASM) |
| **WIT** | Only `raw(list<u8>)` payload supported |
| **State** | Stateless transforms only |
| **Metrics** | In-memory only (no Prometheus export) |
| **Capabilities** | Network/filesystem flags are placeholders |
| **Threading** | `WaferEngine` not `Clone` due to `OnceLock<Linker>` |
| **CLI** | DAG config not exposed via CLI (use DagOrchestrator API) |

---

## Version Reference

| Component | Version | Notes |
|-----------|---------|-------|
| Rust | 1.88.0 | Pinned in `rust-toolchain.toml` |
| wasmtime | 41.0.3 | Host runtime |
| wasmtime-wasi | 41.0.3 | WASI P2 bindings |
| wit-bindgen | 0.53.1 | Guest code generation |
| Target | `wasm32-wasip2` | WASI Preview 2 |
| tokio | 1.x | Async runtime (with `sync`, `time` features) |
| petgraph | 0.6 | DAG topology management |

---

## Running the MVP

```bash
# Build host runtime
cargo build

# Build pass-through plugin
cd plugins/pass-through
cargo build --release --target wasm32-wasip2
cd ../..

# Run single-transform pipeline (stdin/stdout)
echo "hello world" | cargo run -- --config pipeline.toml

# Run tests
cargo test

# Run integration tests specifically
cargo test --test integration
```

### DAG Pipeline Usage

DAG pipelines are currently used via the `DagOrchestrator` API (not CLI):

```rust
use wafer_poc::dag::DagOrchestrator;
use wafer_poc::config::DagConfig;
use wafer_poc::node::{AnyNode, FileSource, FileSink};

// Load config
let config: DagConfig = toml::from_str(config_str)?;

// Create orchestrator
let mut orchestrator = DagOrchestrator::from_config(config)?;

// Register nodes
orchestrator.register_node("source", AnyNode::from_source(FileSource::new("source", "input.txt")))?;
orchestrator.register_node("sink", AnyNode::from_sink(FileSink::new("sink", "output.txt")))?;

// Wire queues and run
orchestrator.wire_queues()?;
orchestrator.run().await?;
```

See `tests/integration.rs` for complete examples.

---

## Architecture Highlights

### Generic I/O Pattern

The `PipelineExecutor` is generic over I/O streams for testability:

```rust
// Production: uses stdin/stdout
let executor = PipelineBuilder::new()
    .with_config(config)
    .build()
    .await?
    .with_stdio();

// Testing: uses in-memory buffers
let input = std::io::Cursor::new(b"test data\n");
let mut output = Vec::new();
let executor = core.with_io(input, &mut output);
```

### Cached Linker

The `WaferEngine` caches the WASI linker via `OnceLock` to avoid expensive re-creation:

```rust
pub fn linker(&self) -> Result<&Linker<WaferState>> {
    // Returns cached linker or creates on first call
}
```

### Capability Scoping

Plugins can be sandboxed with different capability levels:

```rust
// Minimal sandbox (no host access)
WaferState::sandboxed()

// Inherit stdio (default for debugging)
WaferState::with_capabilities(Capabilities::with_stdio())

// Full access (trusted plugins only)
WaferState::with_capabilities(Capabilities::full())
```

---

## MVP Simplifications vs SPEC

This section documents intentional deviations from the full SPEC for MVP simplicity:

| Area | SPEC | MVP Simplification |
|------|------|--------------------|
| **Error codes** | String-based (e.g., "PARSE_FAILED") | String-based (aligned) |
| **Capabilities** | Full network/filesystem scoping | `allow_network`/`allow_filesystem` are placeholders only |
| **Epoch ticker** | Automatic interruption | Requires explicit `engine.start_epoch_ticker()` call |
| **Queue impl** | Unspecified | `tokio::sync::mpsc` async channels |
| **Source/Sink** | WASM components | Host-only Rust implementations |
| **Router/Joiner** | Full support | Not implemented |
| **Hot-swap** | Drain-and-flip | Not implemented |
| **Payload types** | Multiple variants | Only `raw(list<u8>)` |

### Why These Simplifications?

1. **Placeholders over stubs**: Network/filesystem capabilities require careful security design. Placeholders document intent without half-baked implementations.

2. **Explicit over magic**: Epoch ticker requires explicit start to allow different scheduling strategies in the future.

3. **Host-only I/O nodes**: WASM-based Source/Sink would require additional WIT interfaces and component model complexity. Host implementations prove the architecture.

4. **Async-first**: Using `tokio::sync::mpsc` instead of blocking channels ensures the runtime is async-friendly throughout.

---

## See Also

- [SPEC.md](SPEC.md) - Full specification (target design)
- [AI_WORKFLOW.md](AI_WORKFLOW.md) - Development workflow
- [ADRs](adr/) - Architecture Decision Records
