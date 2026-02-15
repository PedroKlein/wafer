# WAFER MVP Documentation

> Current state of the WebAssembly Flow Execution Runtime proof-of-concept

**Version:** 0.1.0  
**Tech Stack:** Rust 1.88+, wasmtime 41.0.3, wit-bindgen 0.53.1, WASI Preview 2

---

## Current State

The MVP implements a **single-transform pipeline** with:
- WASI Preview 2 component loading via wasmtime
- WIT-based type contracts (`transform-node` world)
- Fuel-based execution metering with per-call reset
- Epoch interruption for cooperative scheduling
- Capability-scoped WASI contexts
- Generic I/O for testability
- SPSC bounded queues (not yet integrated into pipeline)
- TOML configuration loading

**What works:** Load a transform plugin, initialize it with full lifecycle (`validate`, `init`, `close`), pipe stdin through `process()`, write to stdout.

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
│   └── mod.rs        # AnyNode enum for heterogeneous DAG storage
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
│   ├── schema.rs     # PipelineConfig, TransformConfig structs
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
- [x] `WasmTransform` struct implementing both traits
- [x] `AnyNode` enum for future heterogeneous DAG storage
- [x] Traits are `Send` but not `Sync` (WASM stores aren't thread-safe)

### Pipeline Execution
- [x] Single transform node execution
- [x] Generic I/O: `PipelineExecutor<R: BufRead, W: Write>`
- [x] `PipelineExecutorCore` builder for deferred I/O attachment
- [x] `.with_io(reader, writer)` for testing
- [x] `.with_stdio()` for production
- [x] ProcessResult handling: `emit`, `filter`, `error`
- [x] Configuration via TOML files

### Capability Scoping
- [x] `Capabilities` struct for security boundaries
- [x] `Capabilities::sandbox()` - minimal access
- [x] `Capabilities::with_stdio()` - inherit stdin/stdout (default)
- [x] `Capabilities::full()` - trusted plugins
- [x] Placeholders for network/filesystem scoping

### Infrastructure
- [x] Bounded SPSC queue (implemented, not integrated)
- [x] RuntimeEnvelope ↔ WIT Envelope conversion
- [x] Atomic metrics: `messages_total`, `process_time_ns`, `queue_depth`
- [x] ProcessTimer RAII guard for timing
- [x] Error types: `WaferError`, `ConfigError` (with `Message` variant)

### Example Plugin
- [x] `pass-through` transform (wit-bindgen 0.53.1)
- [x] Implements full lifecycle: `validate`, `init`, `close`
- [x] Compiles to `wasm32-wasip2` target

---

## What's NOT Implemented

### Node Types (SPEC §5)
- [ ] Source nodes (`poll`, `ack`)
- [ ] Sink nodes (`collect`, `flush`)
- [ ] Router nodes (1→N routing)
- [ ] Joiner nodes (N→1 merge)

### DAG Orchestration
- [ ] Multi-node pipelines
- [ ] Edge wiring between nodes
- [ ] Queue integration for inter-node communication
- [ ] Backpressure propagation

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
| **Pipeline** | Single transform only (no DAG) |
| **I/O** | stdin/stdout only (no MQTT, no files) |
| **WIT** | Only `raw(list<u8>)` payload supported |
| **State** | Stateless transforms only |
| **Queues** | BoundedQueue exists but unused |
| **Metrics** | In-memory only (no Prometheus export) |
| **Capabilities** | Network/filesystem flags are placeholders |
| **Threading** | `WaferEngine` not `Clone` due to `OnceLock<Linker>` |

---

## Version Reference

| Component | Version | Notes |
|-----------|---------|-------|
| Rust | 1.88.0 | Pinned in `rust-toolchain.toml` |
| wasmtime | 41.0.3 | Host runtime |
| wasmtime-wasi | 41.0.3 | WASI P2 bindings |
| wit-bindgen | 0.53.1 | Guest code generation |
| Target | `wasm32-wasip2` | WASI Preview 2 |
| crossbeam-channel | 0.5 | Queue implementation |

---

## Running the MVP

```bash
# Build host runtime
cargo build

# Build pass-through plugin
cd plugins/pass-through
cargo build --release --target wasm32-wasip2
cd ../..

# Run with plugin
echo "hello world" | cargo run -- --config pipeline.toml

# Run tests
cargo test

# Run integration tests specifically
cargo test --test integration
```

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

## See Also

- [SPEC.md](SPEC.md) - Full specification (target design)
- [AI_WORKFLOW.md](AI_WORKFLOW.md) - Development workflow
- [ADRs](adr/) - Architecture Decision Records
