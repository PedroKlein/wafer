# WAFER MVP Documentation

> Current state of the WebAssembly Flow Execution Runtime proof-of-concept

**Version:** 0.3.0  
**Tech Stack:** Rust 1.93, wasmtime (git), wit-bindgen 0.53.1, WASI Preview 2, petgraph 0.8

---

## Current State

The MVP implements **DAG-only pipelines** with:
- Linear chain execution: `StdinSource/FileSource → Transform(s) → StdoutSink/FileSink`
- WASI Preview 2 component loading via wasmtime
- WIT-based type contracts (`transform-node` world)
- Fuel-based execution metering with per-call reset
- Epoch interruption for cooperative scheduling
- Capability-scoped WASI contexts
- **SPSC bounded queues for inter-node communication**
- **petgraph-based DAG topology management**
- **CLI with DAG config loading**

**What works:**
```bash
# stdin/stdout pipeline
echo "hello" | cargo run -- --config examples/dag-uppercase.toml
# Output: HELLO

# File-based pipeline
cargo run -- --config examples/dag-file-io.toml
```

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
│   ├── source.rs     # Source trait + FileSource + StdinSource
│   ├── sink.rs       # Sink trait + FileSink + StdoutSink
│   └── mod.rs        # AnyNode enum (Transform, Source, Sink variants)
├── dag/
│   ├── orchestrator.rs # DagOrchestrator: petgraph topology, queue wiring, async execution
│   └── mod.rs
├── queue/
│   ├── bounded.rs    # BoundedQueue<T>: SPSC with tokio::sync::mpsc
│   ├── envelope.rs   # RuntimeEnvelope: host-side message wrapper
│   └── mod.rs
├── config/
│   ├── loader.rs     # DAG config loading with validation
│   ├── schema.rs     # DagConfig, NodeDefinition, EdgeDefinition
│   └── mod.rs
├── metrics/
│   └── counters.rs   # PipelineMetrics: atomic counters, ProcessTimer
├── error.rs          # WaferError enum, ConfigError, Result type
├── factory.rs        # FactoryContext: node creation from config definitions
├── lib.rs
└── main.rs           # CLI: --config flag, DAG-only execution
```

### WIT Files

```
wit/
├── types.wit         # Shared types: envelope, payload, process-result, metadata
├── lifecycle.wit     # Lifecycle interface: validate, init, close
└── transform.wit     # Transform interface and transform-node world
```

### Plugins

| Plugin | Purpose |
|--------|---------|
| `plugins/pass-through/` | No-op transform (emits input unchanged) |
| `plugins/uppercase/` | ASCII uppercase transform |
| `plugins/json-parse/` | JSON validation and pretty-print |
| `plugins/filter/` | Pattern-based message filtering |
| `plugins/tensor-prep/` | Image normalization (784 bytes → 3136 bytes F32) |
| `plugins/mnist-inference/` | MNIST digit recognition via wasi-nn |
| `plugins/result-format/` | Inference output formatting (logits → JSON) |

### Example Configs

| Config | Description |
|--------|-------------|
| `examples/dag-passthrough.toml` | stdin → passthrough → stdout |
| `examples/dag-uppercase.toml` | stdin → uppercase → stdout |
| `examples/dag-json-parse.toml` | stdin → json-parse → stdout |
| `examples/dag-filter.toml` | stdin → filter(DEBUG) → stdout |
| `examples/dag-chain.toml` | stdin → uppercase → filter → stdout |
| `examples/dag-file-io.toml` | file → passthrough → file |
| `examples/dag-mnist-inference.toml` | file → tensor-prep → mnist-inference → result-format → stdout |

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
- [x] `StdinSource` struct implementing Lifecycle + Source (stdin line reading)
- [x] `StdoutSink` struct implementing Lifecycle + Sink (stdout line writing)
- [x] `AnyNode` enum with `Transform`, `Source`, `Sink` variants
- [x] Traits are `Send` but not `Sync` (WASM stores aren't thread-safe)

### DAG Orchestration (CLI)
- [x] `DagOrchestrator` with petgraph-based topology
- [x] `DagConfig` schema with nodes and edges definitions
- [x] Linear chain execution: `Source → Transform(s) → Sink`
- [x] Topological sort for execution order
- [x] Cycle detection (rejects invalid DAGs)
- [x] Bounded queue wiring between nodes
- [x] Async execution with tokio::spawn per node
- [x] Graceful shutdown in reverse topological order
- [x] Error handling: log and continue (non-fatal)
- [x] CLI `--config` flag for DAG TOML files
- [x] `source_type` discriminator: `stdin` or `file`
- [x] `sink_type` discriminator: `stdout` or `file`

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

### Example Plugins
- [x] `pass-through` transform - No-op, emits input unchanged
- [x] `uppercase` transform - Converts payload to ASCII uppercase (errors on non-UTF8)
- [x] `json-parse` transform - Validates JSON syntax, pretty-prints with 2-space indent
- [x] `filter` transform - Drops messages matching pattern (requires `pattern = "..."` config)
- [x] All plugins implement full lifecycle: `validate`, `init`, `close`
- [x] All compile to `wasm32-wasip2` target (wit-bindgen 0.53.1)

---

## What's NOT Implemented

### Node Types (SPEC §5)
- [x] ~~Source nodes~~ - FileSource + StdinSource implemented (native Rust)
- [x] ~~Sink nodes~~ - FileSink + StdoutSink implemented (native Rust)
- [ ] Router nodes (1→N routing)
- [ ] Joiner nodes (N→1 merge)
- [x] ~~WASM-based Source/Sink~~ - **Decision: Not implementing** - Sources/sinks remain native Rust (see [ADR-0004](adr/0004-native-sources-sinks.md))

### Sink Features (SPEC §4.9)
- [ ] Sink batching: `batch_size` configuration (SPEC §4.9)
- [ ] Sink batching: `batch_timeout` configuration (SPEC §4.9)
- [ ] Sink `flush()` interface for buffered output (SPEC §4.9)

### DAG Orchestration
- [x] ~~Multi-node pipelines~~ - Linear chain implemented
- [x] ~~Edge wiring between nodes~~ - BoundedQueue integration complete
- [x] ~~Queue integration for inter-node communication~~ - Done
- [x] ~~Backpressure propagation~~ - Blocking queues implemented
- [x] ~~CLI integration~~ - `--config` flag for DAG TOML files
- [ ] Fan-out/fan-in topologies (Router/Joiner)
- [ ] Dead Letter Queue (DLQ) routing (SPEC §7.2)

### Dynamic Features
- [ ] Hot-swap (drain-and-flip)
- [ ] Dynamic topology (add/remove nodes)
- [ ] Config file watching
- [ ] REST API for topology changes

### Queue Features (SPEC §8)
- [ ] Overflow policy: `drop` (SPEC §8.2) - only `slow`/blocking exists
- [ ] Overflow policy: `dead-letter` (SPEC §8.2)

### External Integration
- [ ] MQTT source/sink
- [ ] Other external connectors

### Advanced WIT Types
- [ ] `json-value` payload variant
- [ ] `sensor-reading` payload variant
- [ ] `tensor` payload variant (wasi-nn)
- [ ] `image-data` payload variant

### Capabilities
- [x] wasi-nn inference (MNIST demo)
- [ ] Host-managed node state
- [ ] Network capability enforcement (currently placeholder)
- [ ] Filesystem capability enforcement (currently placeholder)

### Observability (SPEC §12)
- [ ] Prometheus `/metrics` endpoint (SPEC §12.3) - metrics are in-memory only
- [ ] Health endpoints: `/health`, `/ready`, `/live` (SPEC §12.5)
- [ ] Structured JSON log output (SPEC §12.4) - tracing uses text format

---

## Known Limitations

| Area | Limitation | Notes |
|------|------------|-------|
| **Pipeline** | Linear chains only (no fan-out/fan-in) | Router/Joiner pending |
| **I/O** | stdin/stdout or file-based (no network sources) | MQTT planned |
| **Source/Sink** | Native Rust only (by design) | See [ADR-0004](adr/0004-native-sources-sinks.md) |
| **WIT** | Only `raw(list<u8>)` payload supported | |
| **State** | Stateless transforms only | |
| **Metrics** | In-memory only (no Prometheus export) | |
| **Logging** | Text format only (no JSON export) | |
| **Error handling** | No DLQ, errors logged only | |
| **Capabilities** | Network/filesystem flags are placeholders | |
| **Threading** | `WaferEngine` not `Clone` due to `OnceLock<Linker>` | |
| **WASI-NN** | ONNX backend only, CPU-only (no GPU acceleration) | |

---

## Version Reference

| Component | Version | Notes |
|-----------|---------|-------|
| Rust | 1.93 | Pinned in `rust-toolchain.toml` (min 1.75) |
| wasmtime | git rev | Post-41.0.3 with ONNX fix (see Cargo.toml) |
| wasmtime-wasi | git rev | WASI P2 bindings |
| wasmtime-wasi-nn | git rev | ONNX backend for ML inference |
| wit-bindgen | 0.53.1 | Guest code generation |
| Target | `wasm32-wasip2` | WASI Preview 2 |
| tokio | 1.x | Async runtime (with `sync`, `time`, `signal` features) |
| petgraph | 0.8 | DAG topology management |
| tokio-util | 0.7 | CancellationToken for graceful shutdown |

---

## Running the MVP

```bash
# Build host runtime
cargo build

# Build all plugins
just plugin

# Run DAG pipeline with stdin/stdout
echo "hello world" | cargo run -- --config examples/dag-passthrough.toml

# Run with uppercase transform
echo "hello" | cargo run -- --config examples/dag-uppercase.toml
# Output: HELLO

# Run tests
cargo test

# Run integration tests specifically
cargo test --test integration
```

### DAG Configuration

DAG pipelines are configured via TOML files:

```toml
[[nodes]]
id = "source"
node_type = "source"
source_type = "stdin"  # or "file" with config.path

[[nodes]]
id = "transform"
node_type = "transform"
config = { plugin_path = "plugins/uppercase/target/wasm32-wasip2/release/uppercase_transform.wasm" }

[[nodes]]
id = "sink"
node_type = "sink"
sink_type = "stdout"  # or "file" with config.path

[[edges]]
from = "source"
to = "transform"

[[edges]]
from = "transform"
to = "sink"
```

See `examples/` directory for complete examples.

---

## WASI-NN Inference

The MVP supports ML inference pipelines via **wasi-nn** integration using the ONNX backend.

### Inference Pipeline

The wasi-nn inference pipeline processes image data through a chain of specialized transforms:

```
Input (28x28 grayscale) → tensor-prep → mnist-inference → result-format → JSON Output
        784 bytes         → 3136 bytes  →    40 bytes    →   JSON
```

**Components:**
- **wasmtime-wasi-nn**: Host runtime integration for the wasi-nn standard
- **ONNX backend**: CPU-only execution using ONNX Runtime (via `ort` crate)
- **Embedded model**: MNIST-8.onnx (~26KB) included via `include_bytes!`

### Running MNIST Inference

```bash
# Run MNIST inference pipeline
cargo run -- --config examples/dag-mnist-inference.toml

# Output: {"digit": 7, "confidence": 0.95, "all_scores": [...]}
```

### Inference Plugins

| Plugin | Input | Output | Description |
|--------|-------|--------|-------------|
| `tensor-prep` | 784 bytes (28x28 u8) | 3136 bytes (784 × F32) | Normalizes grayscale pixels to F32 tensor |
| `mnist-inference` | 3136 bytes (F32 tensor) | 40 bytes (10 × F32) | Digit recognition via embedded MNIST-8 model |
| `result-format` | 40 bytes (10 × F32) | JSON string | Converts logits to `{digit, confidence, all_scores}` |

### Data Flow Details

1. **Input**: 28×28 grayscale image as 784 raw bytes (row-major, 0-255)
2. **tensor-prep**: Converts each byte to F32, normalizes to [0.0, 1.0] range → 3136 bytes
3. **mnist-inference**: Runs ONNX model inference, outputs 10 logits (one per digit) → 40 bytes
4. **result-format**: Applies softmax, finds argmax, formats as JSON

**Example output:**
```json
{"digit": 7, "confidence": 0.9832, "all_scores": [0.001, 0.002, 0.003, 0.001, 0.001, 0.002, 0.003, 0.983, 0.002, 0.002]}
```

---

## Architecture Highlights

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

| Area | SPEC | MVP Simplification |
|------|------|--------------------
| **Error codes** | String-based (e.g., "PARSE_FAILED") | String-based (aligned) |
| **Capabilities** | Full network/filesystem scoping | `allow_network`/`allow_filesystem` are placeholders only |
| **Epoch ticker** | Automatic interruption | Requires explicit `engine.start_epoch_ticker()` call |
| **Queue impl** | Unspecified | `tokio::sync::mpsc` async channels |
| **Source/Sink** | Native Rust (ADR-0004) | Native Rust (aligned) |
| **Router/Joiner** | Full support | Not implemented |
| **Hot-swap** | Drain-and-flip | Not implemented |
| **Payload types** | Multiple variants | Only `raw(list<u8>)` |

---

## See Also

- [SPEC.md](SPEC.md) - Full specification (target design)
- [AI_WORKFLOW.md](AI_WORKFLOW.md) - Development workflow
- [ADRs](adr/) - Architecture Decision Records
