# WAFER MVP Documentation

> Current state of the WebAssembly Flow Execution Runtime proof-of-concept

**Version:** 0.1.0  
**Tech Stack:** Rust 1.88+, wasmtime 41.0.3, wit-bindgen 0.53.1, WASI Preview 2

---

## Current State

The MVP implements a **single-transform pipeline** with:
- WASI Preview 2 component loading via wasmtime
- WIT-based type contracts (`transform-node` world)
- Fuel-based execution metering
- SPSC bounded queues (not yet integrated into pipeline)
- TOML configuration loading

**What works:** Load a transform plugin, initialize it, pipe stdin through `process()`, write to stdout.

---

## Module Structure

```
src/
├── engine/
│   ├── host.rs       # WaferState: WasiView impl, WASI context
│   ├── instance.rs   # TransformInstance: bindgen!, component instantiation
│   ├── loader.rs     # WaferEngine: wasmtime config, component loading
│   └── mod.rs
├── pipeline/
│   ├── executor.rs   # PipelineExecutor: stdin→process()→stdout loop
│   ├── builder.rs    # PipelineBuilder: config→executor construction
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
├── error.rs          # WaferError enum, Result type
├── lib.rs
└── main.rs           # CLI: --config flag, pipeline.toml
```

### External Files

| File | Purpose |
|------|---------|
| `wit/transform.wit` | WIT contract for transform nodes |
| `plugins/pass-through/` | Example transform (no-op, emits input unchanged) |

---

## What's Implemented

### Core Runtime
- [x] Wasmtime engine with component model support
- [x] WASI Preview 2 via `wasmtime-wasi` P2 bindings
- [x] Async support (`async_support(true)`)
- [x] Fuel metering (default: 1,000,000 per call)
- [x] Component loading from `.wasm` files

### WIT Contract (`transform-node` world)
- [x] `types` interface: `Envelope`, `ProcessResult`, `MetadataEntry`
- [x] `lifecycle` interface: `init(NodeConfig)`
- [x] `transform` interface: `process(Envelope) -> ProcessResult`
- [x] Payload variant: `raw(list<u8>)` only

### Pipeline Execution
- [x] Single transform node execution
- [x] stdin→transform→stdout data flow
- [x] ProcessResult handling: `emit`, `filter`, `error`
- [x] Configuration via TOML files
- [x] Per-call fuel refill when exhausted

### Infrastructure
- [x] Bounded SPSC queue (implemented, not integrated)
- [x] RuntimeEnvelope ↔ WIT Envelope conversion
- [x] Atomic metrics: `messages_total`, `process_time_ns`, `queue_depth`
- [x] ProcessTimer RAII guard for timing
- [x] Error types: `WaferError`, `ConfigError`

### Example Plugin
- [x] `pass-through` transform (wit-bindgen 0.53.1)
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
- [ ] Full `process-error` with original envelope

### Capabilities
- [ ] wasi-nn inference
- [ ] Host-managed node state
- [ ] Capability-scoped isolation beyond stdio

---

## Known Limitations

| Area | Limitation |
|------|------------|
| **Pipeline** | Single transform only (no DAG) |
| **I/O** | stdin/stdout only (no MQTT, no files) |
| **WIT** | Only `raw(list<u8>)` payload supported |
| **Lifecycle** | No `validate()` or `close()` in WIT |
| **State** | Stateless transforms only |
| **Queues** | BoundedQueue exists but unused |
| **Metrics** | In-memory only (no Prometheus export) |

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
```

---

## See Also

- [SPEC.md](SPEC.md) - Full specification (target design)
- [AI_WORKFLOW.md](AI_WORKFLOW.md) - Development workflow
- [ADRs](adr/) - Architecture Decision Records
