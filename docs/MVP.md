# WAFER MVP Documentation

> Current state of the WebAssembly Flow Execution Runtime proof-of-concept

**Version:** 0.4.0  
**Tech Stack:** Rust 1.93.1, wasmtime (git main branch), wit-bindgen 0.53.1, WASI Preview 2, petgraph 0.8  
**Last Updated:** 2026-02-28

---

## Current State

The MVP implements **DAG pipelines with fan-out/fan-in support** plus a **runtime control plane**:

### Core Pipeline Features
- Linear chain execution: `StdinSource/FileSource/MqttSource → Transform(s) → StdoutSink/FileSink/MqttSink`
- **Router nodes (1→N)**: Content-based routing via `WasmRouter` and `router-node` WIT world
- **Joiner nodes (N→1)**: Merge support via `WasmJoiner` and `joiner-node` WIT world
- **Fan-out/Fan-in topologies**: Diamond patterns, scatter-gather workflows
- WASI Preview 2 component loading via wasmtime
- WIT-based type contracts (`transform-node`, `router-node`, `joiner-node` worlds)
- Fuel-based execution metering with per-call reset
- Epoch interruption for cooperative scheduling
- Capability-scoped WASI contexts
- **SPSC bounded queues for inter-node communication**
- **Queue overflow policies**: `slow` (backpressure), `drop`, `dead-letter`
- **Dead Letter Queue (DLQ)**: Pipeline-wide sink for failed/dropped messages
- **Sink batching**: `batch_size`, `batch_timeout_ms`, and `flush()` support
- **petgraph-based DAG topology management**
- **CLI with DAG config loading**

### Control Plane (NEW in 0.4.0)
- **Workspace restructure**: `wafer-core`, `wafer-types`, `wafer-runtime`, `waferctl` crates
- **HTTP API server** (Axum): Health, pipeline status, node info, and control endpoints
- **`waferctl` CLI**: Management tool for interacting with running runtimes
- **Prometheus metrics endpoint**: `/metrics` with pipeline and node counters
- **PipelineControl trait**: Unified interface for runtime management

**What works:**
```bash
# stdin/stdout pipeline
echo "hello" | cargo run -p wafer-runtime -- --config examples/dag-uppercase.toml
# Output: HELLO

# File-based pipeline
cargo run -p wafer-runtime -- --config examples/dag-file-io.toml

# Start runtime with HTTP API (default port 8080)
cargo run -p wafer-runtime -- --config examples/dag-passthrough-with-api.toml

# Use waferctl to check status
waferctl health
waferctl status
waferctl nodes
```

---

## Module Structure

```
crates/
├── wafer-core/           # Core runtime library
│   └── src/
│       ├── api/          # HTTP API (feature-gated with `http-api`)
│       │   ├── mod.rs
│       │   ├── server.rs   # Axum router and middleware
│       │   ├── handlers.rs # Endpoint implementations
│       │   └── metrics.rs  # Prometheus metrics handler
│       ├── control/      # Pipeline control interface
│       │   ├── mod.rs
│       │   └── traits.rs   # PipelineControl trait definition
│       ├── engine/
│       │   ├── mod.rs
│       │   ├── host.rs         # WaferState: WasiView impl, Capabilities
│       │   ├── instance.rs     # TransformInstance: bindgen!, instantiation
│       │   ├── loader.rs       # WaferEngine: wasmtime config, linker
│       │   └── capabilities.rs # Capabilities struct for security
│       ├── node/
│       │   ├── mod.rs
│       │   ├── traits.rs     # Lifecycle, Transform, Router, Joiner traits
│       │   ├── transform.rs  # WasmTransform
│       │   ├── router.rs     # WasmRouter: 1→N routing
│       │   ├── joiner.rs     # WasmJoiner: N→1 merge
│       │   ├── source/       # Source implementations (stdin, file, mqtt, http)
│       │   └── sink/         # Sink implementations (stdout, file, mqtt, http)
│       ├── dag/
│       │   ├── mod.rs
│       │   ├── orchestrator.rs   # DagOrchestrator: run(), topology
│       │   ├── builder.rs        # from_config(), validation
│       │   ├── runner.rs         # Node execution loops
│       │   ├── metrics_helper.rs # Metrics recording helpers
│       │   ├── result_handler.rs # ProcessResult/RouteResult handling
│       │   └── control.rs        # PipelineControl implementation
│       ├── queue/
│       │   ├── mod.rs
│       │   ├── bounded.rs    # BoundedQueue<T>: SPSC via tokio::sync::mpsc
│       │   └── envelope.rs   # RuntimeEnvelope
│       ├── config/
│       │   ├── mod.rs
│       │   ├── loader.rs     # DAG config loading
│       │   └── schema.rs     # DagConfig, NodeDefinition, ApiConfig, MetricsConfig
│       ├── registry/
│       │   ├── mod.rs
│       │   ├── cache.rs      # PackageCache: TTL-based
│       │   ├── client.rs     # WaferRegistry: OCI client
│       │   └── types.rs      # PluginSource, OciReference
│       ├── metrics/
│       │   ├── mod.rs
│       │   └── counters.rs   # PipelineMetrics
│       ├── error.rs
│       ├── factory.rs
│       └── lib.rs
├── wafer-types/          # Shared API types (dependency-free)
│   └── src/
│       ├── lib.rs
│       ├── control.rs      # PipelineStatus, NodeInfo, ControlError, HotSwapResult, etc.
│       ├── events.rs       # PipelineEvent enum for event streaming
│       └── metrics.rs      # MetricsSnapshot for API responses
├── wafer-runtime/        # Runtime binary
│   └── src/
│       └── main.rs         # CLI with --config, --api-bind, --no-api flags
│   └── tests/
│       └── integration.rs  # API integration tests
└── waferctl/             # CLI management tool
    └── src/
        ├── main.rs         # Command handlers (clap-based)
        ├── client.rs       # WaferClient HTTP client (reqwest)
        ├── config.rs       # Endpoint configuration (~/.config/waferctl/)
        ├── error.rs        # Exit codes, error formatting
        └── output.rs       # Table formatting (tabled crate)
```

### WIT Files

```
wit/
├── types.wit         # Shared types: envelope, payload, process-result, metadata
├── lifecycle.wit     # Lifecycle interface: validate, init, close
├── transform.wit     # Transform interface and transform-node world
├── router.wit        # Router interface and router-node world (1→N routing)
└── joiner.wit        # Joiner interface and joiner-node world (N→1 merge)
```

### Plugins (9 total)

| Plugin | Type | Purpose | Target |
|--------|------|---------|--------|
| `plugins/pass-through/` | Transform | No-op transform (emits input unchanged) | wasm32-wasip2 |
| `plugins/uppercase/` | Transform | ASCII uppercase transform | wasm32-wasip2 |
| `plugins/json-parse/` | Transform | JSON validation and pretty-print | wasm32-wasip2 |
| `plugins/filter/` | Transform | Pattern-based message filtering | wasm32-wasip2 |
| `plugins/tensor-prep/` | Transform | Image normalization (784 bytes → 3136 bytes F32) | wasm32-wasip2 |
| `plugins/mnist-inference/` | Transform | MNIST digit recognition via wasi-nn | wasm32-wasip2 |
| `plugins/result-format/` | Transform | Inference output formatting (logits → JSON) | wasm32-wasip2 |
| `plugins/content-router/` | Router | Content-based 1→N routing (routes by JSON `route` field) | wasm32-wasip2 |
| `plugins/merge-joiner/` | Joiner | Stateless N→1 merge (passes through all inputs) | wasm32-wasip2 |

### Example Configs (17 total)

| Config | Description |
|--------|-------------|
| `examples/dag-passthrough.toml` | stdin → passthrough → stdout |
| `examples/dag-uppercase.toml` | stdin → uppercase → stdout |
| `examples/dag-json-parse.toml` | stdin → json-parse → stdout |
| `examples/dag-filter.toml` | stdin → filter(DEBUG) → stdout |
| `examples/dag-chain.toml` | stdin → uppercase → filter → stdout |
| `examples/dag-file-io.toml` | file → passthrough → file |
| `examples/dag-mnist-inference.toml` | file → tensor-prep → mnist-inference → result-format → stdout |
| `examples/dag-mqtt.toml` | mqtt → passthrough → mqtt |
| `examples/dag-mqtt-simple.toml` | mqtt → mqtt (no transform) |
| `examples/dag-remote.toml` | stdin → OCI plugin → stdout |
| `examples/dag-diamond.toml` | Diamond/scatter-gather: source → router → transforms → joiner → sink |
| `examples/dag-fanout.toml` | Fan-out: source → router → multiple sinks |
| `examples/dag-passthrough-with-api.toml` | Passthrough with `[api]` and `[metrics]` config sections |
| `examples/dag-metrics-demo.toml` | Metrics/observability demo with Prometheus endpoint |
| `examples/dag-overflow-dlq-demo.toml` | Overflow policies (`slow`, `drop`, `dead-letter`), DLQ, and sink batching |
| `examples/dag-http.toml` | HTTP webhook source → transform → HTTP POST sink |

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
- [x] Component loading from bytes (`load_component_from_bytes`)

### Registry Support (OCI)
- [x] `WaferRegistry` client for fetching plugins from OCI registries
- [x] Direct OCI image references (e.g., `ghcr.io/pedroklein/wafer-uppercase:0.0.1`)
- [x] File-based cache with TTL-based invalidation (`~/.cache/wafer/plugins/`)
- [x] Docker credential support via `docker_credential` crate (config.json + helpers)
- [x] `--no-cache` CLI flag to bypass cache
- [x] Fallback to cached version on network error (with warning)
- [x] `RegistryConfig` for TTL and cache directory

### WIT Contracts
- [x] `types` interface: `Envelope`, `Payload`, `ProcessResult`, `ProcessError`, `Metadata`
- [x] `lifecycle` interface: `validate(NodeConfig)`, `init(NodeConfig)`, `close()`
- [x] `transform` interface: `process(Envelope) -> ProcessResult`
- [x] `router` interface: `output-ports() -> list<string>`, `route(Envelope) -> RouteResult`
- [x] `joiner` interface: `input-ports() -> list<string>`, `join(port, Envelope) -> ProcessResult`
- [x] `transform-node` world for transform plugins
- [x] `router-node` world for router plugins (1→N routing)
- [x] `joiner-node` world for joiner plugins (N→1 merge)
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
- [x] `MqttSource` struct implementing Lifecycle + Source (MQTT subscription via rumqttc)
- [x] `MqttSink` struct implementing Lifecycle + Sink (MQTT publishing via rumqttc)
- [x] `HttpSource` struct implementing Lifecycle + Source (HTTP webhook receiver via hyper)
- [x] `HttpSink` struct implementing Lifecycle + Sink (HTTP POST with batching via reqwest)
- [x] `Router` trait: `output_ports`, `route` async methods
- [x] `Joiner` trait: `input_ports`, `process` async methods
- [x] `WasmRouter` struct implementing Lifecycle + Router
- [x] `WasmJoiner` struct implementing Lifecycle + Joiner
- [x] `AnyNode` enum with `Transform`, `Source`, `Sink`, `Router`, `Joiner` variants
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
- [x] `source_type` discriminator: `stdin`, `file`, `mqtt`, or `http`
- [x] `sink_type` discriminator: `stdout`, `file`, `mqtt`, or `http`

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

## What's NOT Implemented (Roadmap)

### Node Types (SPEC §5) - ✅ COMPLETE
- [x] ~~Source nodes~~ - FileSource + StdinSource + MqttSource implemented (native Rust)
- [x] ~~Sink nodes~~ - FileSink + StdoutSink + MqttSink implemented (native Rust)
- [x] ~~Router nodes (1→N routing)~~ - WasmRouter implemented with `router-node` WIT world
- [x] ~~Joiner nodes (N→1 merge)~~ - WasmJoiner implemented with `joiner-node` WIT world
- [x] ~~WASM-based Source/Sink~~ - **Decision: Not implementing** - Sources/sinks remain native Rust (see [ADR-0004](adr/0004-native-sources-sinks.md))

### Sink Features (SPEC §4.9) - ✅ COMPLETE
- [x] Sink batching: `batch_size` configuration
- [x] Sink batching: `batch_timeout_ms` configuration  
- [x] Sink `flush()` interface for buffered output
- [x] `BatchBuffer<T>` utility for managing batched writes

### DAG Orchestration - ✅ COMPLETE
- [x] ~~Multi-node pipelines~~ - Linear chain implemented
- [x] ~~Edge wiring between nodes~~ - BoundedQueue integration complete
- [x] ~~Queue integration for inter-node communication~~ - Done
- [x] ~~Backpressure propagation~~ - Blocking queues implemented
- [x] ~~CLI integration~~ - `--config` flag for DAG TOML files
- [x] ~~Fan-out/fan-in topologies~~ - Router/Joiner implemented (diamond, scatter-gather patterns)
- [x] Dead Letter Queue (DLQ) routing (SPEC §7.2) - DlqFileSink with envelope metadata

### Control Plane (runtime-control-plane change) - ✅ MOSTLY COMPLETE
- [x] Workspace restructure: `wafer-core`, `wafer-types`, `wafer-runtime`, `waferctl`
- [x] HTTP API server (Axum) with health, pipeline, and node endpoints
- [x] `waferctl` CLI with commands: status, nodes, hot-swap, reload, drain, shutdown
- [x] PipelineControl trait for runtime management
- [x] API endpoint implementations (handlers wired to PipelineControl)
- [ ] Hot-swap implementation (stubs return `NotImplemented` - see hot-swap-mechanism)
- [ ] E2E tests with long-running fixtures (deferred)

### Hot-Swap Mechanism (hot-swap-mechanism change) - ✅ COMPLETE
- [x] Node state machine (Starting, Running, Draining, Retired) - `NodeStateTracker` with atomic state transitions
- [x] Message routing control (pause/resume) - `RoutingController` with buffering
- [x] HotSwapCoordinator (prepare, drain, flip, retire phases) - Full drain-and-flip algorithm
- [x] Drain timeout handling - Configurable timeout with forced swap fallback
- [x] Config diff & resync - `ConfigDiff` struct, `diff_configs()`, `resync()` method
- [x] Swap validation & safety - Per-node swap locks, component validation before drain

### Observability (observability-prometheus change) - ✅ COMPLETE
- [x] Prometheus `/metrics` endpoint with full metrics (SPEC §12.2 compliant)
- [x] Health endpoints: `/health`, `/ready` (SPEC §12.5)
- [x] MetricsRegistry with prometheus-client crate (feature-gated under `http-api`)
- [x] System metrics (CPU, memory, threads via sysinfo crate)
- [x] Structured JSON log output via `--log-format json` CLI flag (SPEC §12.4)
- [x] Full metrics wiring (pipeline, node, queue metrics to Prometheus registry)

### Dynamic Features
- [x] ~~OCI registry package fetching~~ - Implemented via `WaferRegistry`
- [x] ~~Hot-swap (drain-and-flip)~~ - Implemented via `HotSwapCoordinator` (see ADR-0003)
- [ ] Dynamic topology (add/remove nodes at runtime)
- [ ] Config file watching (inotify/kqueue)
- [ ] REST API for topology changes

### Queue Features (SPEC §8) - ✅ COMPLETE
- [x] Overflow policy: `slow` (blocking/backpressure)
- [x] Overflow policy: `drop` - discard messages when queue is full
- [x] Overflow policy: `dead-letter` - route dropped messages to DLQ

### External Integration
- [x] ~~MQTT source/sink~~ - Implemented via rumqttc (see [mqtt-setup.md](mqtt-setup.md))
- [ ] HTTP source/sink
- [ ] Kafka source/sink

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

### Future Ideas
- [ ] Support multiple sources/sinks per DAG (currently single source/sink)
- [ ] Plugin hot-reload from OCI registry (watch for new versions)
- [ ] Distributed runtime (multiple wafer-runtime instances)

---

## Known Limitations

| Area | Limitation | Notes |
|------|------------|-------|
| **I/O** | stdin/stdout, file, MQTT, HTTP | Kafka planned |
| **Source/Sink** | Native Rust only (by design) | See [ADR-0004](adr/0004-native-sources-sinks.md) |
| **WIT** | Only `raw(list<u8>)` payload supported | Additional variants planned |
| **State** | Stateless transforms only | Host-managed state planned |
| **Metrics** | Full Prometheus registry | All SPEC §12.2 metrics available |
| **Logging** | JSON and text formats via `--log-format` | Structured logging complete |
| **Error handling** | DLQ for overflow and process errors | Full DLQ support implemented |
| **Hot-swap** | Transform nodes only (sources/sinks not swappable) | By design - see ADR-0003 |
| **Capabilities** | Network/filesystem flags are placeholders | Enforcement not implemented |
| **Threading** | `WaferEngine` not `Clone` due to `OnceLock<Linker>` | By design |
| **WASI-NN** | ONNX backend only, CPU-only (no GPU acceleration) | Hardware backends planned |

---

## Version Reference

| Component | Version | Notes |
|-----------|---------|-------|
| Rust | 1.93.1 | Pinned in `rust-toolchain.toml` (min 1.75) |
| wasmtime | git main | Post-41.0.3 with ONNX fix and PR #12379 async fix |
| wasmtime-wasi | git main | WASI P2 bindings |
| wasmtime-wasi-nn | git main | ONNX backend for ML inference |
| wit-bindgen | 0.53.1 | Guest code generation |
| Target | `wasm32-wasip2` | WASI Preview 2 |
| tokio | 1.x | Async runtime (with `sync`, `time`, `signal` features) |
| petgraph | 0.8 | DAG topology management |
| tokio-util | 0.7 | CancellationToken for graceful shutdown |
| axum | 0.8 | HTTP API server (feature-gated) |
| prometheus-client | 0.23 | Metrics formatting (feature-gated) |
| reqwest | 0.12 | HTTP client for waferctl |
| clap | 4.x | CLI argument parsing |
| tabled | 0.18 | Table output formatting |

---

## Running the MVP

```bash
# Build entire workspace
just build
# Or: cargo build --workspace

# Build all plugins
just plugin

# Run DAG pipeline with stdin/stdout
echo "hello world" | just run examples/dag-passthrough.toml
# Or: cargo run -p wafer-runtime -- --config examples/dag-passthrough.toml

# Run with uppercase transform
echo "hello" | just run examples/dag-uppercase.toml
# Output: HELLO

# Run with HTTP API enabled
cargo run -p wafer-runtime -- --config examples/dag-passthrough-with-api.toml
# API available at http://localhost:8080

# Run all tests
just test
# Or: cargo test --workspace

# Run integration tests specifically
cargo test --test integration -p wafer-runtime
```

### Using waferctl

```bash
# Build waferctl
just build-ctl
# Or: cargo build -p waferctl

# Check runtime health (default: localhost:8080)
waferctl health
# Or: just ctl health

# Use a specific endpoint
waferctl -e http://localhost:9090 status

# List all nodes
waferctl nodes

# Get pipeline status
waferctl status

# Trigger hot-swap on a node (reloads config and swaps to new WASM)
waferctl hot-swap transform-1

# Hot-swap with explicit WASM path
waferctl hot-swap transform-1 --path plugins/uppercase/v2.wasm

# Get metrics (human-readable)
waferctl metrics

# Get metrics (raw Prometheus format)
waferctl metrics --raw
```

### waferctl Configuration

```bash
# Set a named endpoint
waferctl config set-endpoint prod http://prod-wafer:8080

# List configured endpoints
waferctl config list

# Use a named endpoint
waferctl config use prod
waferctl status  # Now uses prod endpoint
```

### DAG Configuration

DAG pipelines are configured via TOML files:

```toml
# Optional: API server configuration
[api]
enabled = true
bind = "0.0.0.0:8080"

# Optional: Separate metrics server (if different from API)
[metrics]
enabled = true
bind = "0.0.0.0:9090"
path = "/metrics"

[[nodes]]
id = "source"
node_type = "source"
source_type = "stdin"  # or "file" with config.path, or "mqtt"

[[nodes]]
id = "transform"
node_type = "transform"
config = { plugin_path = "plugins/uppercase/target/wasm32-wasip2/release/uppercase_transform.wasm" }

[[nodes]]
id = "sink"
node_type = "sink"
sink_type = "stdout"  # or "file" with config.path, or "mqtt"

[[edges]]
from = "source"
to = "transform"

[[edges]]
from = "transform"
to = "sink"
```

See `examples/` directory for complete examples (15 configurations).

### Remote Plugin Configuration (OCI)

Transforms can be loaded from OCI registries instead of local files using direct image references:

```toml
# Registry configuration (optional - has defaults)
[registry]
cache_ttl_hours = 24
# cache_dir = "/custom/cache/path"  # optional

[[nodes]]
id = "transform"
node_type = "transform"
[nodes.config]
oci = "ghcr.io/pedroklein/wafer-uppercase:0.0.1"  # direct OCI reference
```

**CLI flags:**
```bash
# Normal operation (uses cache)
cargo run -- --config examples/dag-remote.toml

# Bypass cache (always fetch from registry)
cargo run -- --config examples/dag-remote.toml --no-cache
```

**Authentication:**
- Reads credentials from `~/.docker/config.json` automatically
- Supports credential helpers (e.g., `docker-credential-osxkeychain`)
- Falls back to anonymous access if no credentials found

**Cache behavior:**
- Plugins cached at `~/.cache/wafer/plugins/{registry}/{repository}/{tag}.wasm`
- TTL-based invalidation (default: 24 hours)
- On network error: falls back to cached version with warning log

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
|------|------|--------------------|
| **Error codes** | String-based (e.g., "PARSE_FAILED") | String-based (aligned) |
| **Capabilities** | Full network/filesystem scoping | `allow_network`/`allow_filesystem` are placeholders only |
| **Epoch ticker** | Automatic interruption | Requires explicit `engine.start_epoch_ticker()` call |
| **Queue impl** | Unspecified | `tokio::sync::mpsc` async channels |
| **Source/Sink** | Native Rust (ADR-0004) | Native Rust (aligned) |
| **Router/Joiner** | Full support | Implemented (content-router, merge-joiner) |
| **Hot-swap** | Drain-and-flip | Fully implemented via HotSwapCoordinator |
| **Payload types** | Multiple variants | Only `raw(list<u8>)` |
| **Control API** | Full REST API | Fully implemented with PipelineControl |
| **Metrics** | Full Prometheus registry | Fully implemented (SPEC §12.2 compliant) |

---

## See Also

- [SPEC.md](SPEC.md) - Full specification (target design)
- [api.md](api.md) - HTTP API documentation
- [REGISTRY.md](REGISTRY.md) - OCI registry plugin support
- [mqtt-setup.md](mqtt-setup.md) - MQTT source/sink setup
- [AI_WORKFLOW.md](AI_WORKFLOW.md) - Development workflow
- [ADRs](adr/) - Architecture Decision Records:
  - [ADR-0001](adr/0001-wasmtime-runtime.md) - Wasmtime runtime selection
  - [ADR-0002](adr/0002-spsc-bounded-queues.md) - SPSC bounded queue design
  - [ADR-0003](adr/0003-drain-and-flip-hotswap.md) - Drain-and-flip hot-swap strategy
  - [ADR-0004](adr/0004-native-sources-sinks.md) - Native sources/sinks decision
  - [ADR-0005](adr/0005-registry-package-support.md) - OCI registry support
  - [ADR-0006](adr/0006-workspace-architecture.md) - Workspace architecture split

## OpenSpec Changes (Implementation Roadmap)

| Change | Status | Description |
|--------|--------|-------------|
| `runtime-control-plane` | ✅ Complete | Workspace restructure, HTTP API, waferctl CLI |
| `observability-prometheus` | ✅ Complete | Full Prometheus metrics, structured logging |
| `hot-swap-mechanism` | ✅ Complete | Drain-and-flip hot-swap implementation |
| `queue-overflow-and-dlq` | ✅ Complete | Overflow policies (drop, dead-letter), DLQ sink, sink batching |

See `openspec/changes/` for detailed task breakdowns.
