# WAFER

**WebAssembly Flow Execution Runtime**

A high-performance, Rust-based DAG pipeline runtime that executes WebAssembly plugins using [Wasmtime](https://wasmtime.dev/). Designed for building data processing pipelines with hot-swappable transforms, bounded queues with backpressure, and flexible fan-out/fan-in topologies.

[![Rust](https://img.shields.io/badge/rust-1.93%2B-orange.svg)](https://www.rust-lang.org/)
[![License](https://img.shields.io/badge/license-MIT%2FApache--2.0-blue.svg)](LICENSE)

## Features

- **DAG-based pipelines** — Define data flows as directed acyclic graphs with TOML configuration
- **WebAssembly plugins** — Write transforms in any language that compiles to WASM (Rust, Go, C/C++, etc.)
- **Hot-swappable transforms** — Update WASM plugins at runtime without pipeline restart
- **Bounded queues** — SPSC queues with configurable capacity, overflow policies (`slow`, `drop`, `dead-letter`), and DLQ support
- **Fan-out/Fan-in** — Router (1→N) and Joiner (N→1) nodes for complex topologies
- **Multiple I/O types** — stdin/stdout, files, MQTT pub/sub (all with optional batching)
- **HTTP Control Plane** — REST API for monitoring and management
- **Prometheus Metrics** — Built-in metrics export for observability
- **OCI Registry Support** — Load plugins from container registries (ghcr.io, Docker Hub)
- **Fuel-based Metering** — Execution limits for untrusted plugins
- **WASI Preview 2** — Modern WebAssembly System Interface support

## Table of Contents

- [Quick Start](#quick-start)
- [Installation](#installation)
- [Project Structure](#project-structure)
- [Development Setup](#development-setup)
- [Running Pipelines](#running-pipelines)
- [Building Plugins](#building-plugins)
- [Control Plane API](#control-plane-api)
- [Configuration Reference](#configuration-reference)
- [Documentation](#documentation)
- [Contributing](#contributing)
- [License](#license)

## Quick Start

```bash
# Clone and build
git clone https://github.com/PedroKlein/wafer-poc.git
cd wafer-poc
cargo build --workspace

# Build example plugins
just build-plugins

# Run a simple pipeline (stdin → uppercase transform → stdout)
echo "hello world" | cargo run -p wafer-runtime -- --config examples/dag-uppercase.toml
# Output: HELLO WORLD
```

## Installation

### Prerequisites

- **Rust 1.93+** (with `wasm32-wasip2` target)
- **just** (command runner) — `cargo install just` or `brew install just`
- **wasm-tools** (optional, for validation) — `cargo install wasm-tools`
- **wkg** (optional, for OCI publishing) — `cargo install wkg`

### Setup

```bash
# Install Rust (if needed)
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh

# The rust-toolchain.toml will automatically configure:
# - Rust 1.93
# - wasm32-wasip2 target
# - rustfmt and clippy

# Verify setup
rustup show
cargo --version
```

## Project Structure

```
wafer-poc/
├── crates/
│   ├── wafer-core/       # Core library: DAG orchestrator, engine, nodes, queues
│   ├── wafer-runtime/    # Binary: CLI runtime with API server integration
│   ├── wafer-types/      # Shared types: errors, control messages, metrics
│   └── waferctl/         # Binary: CLI tool for interacting with running pipelines
├── plugins/              # WebAssembly plugin examples
│   ├── pass-through/     # No-op passthrough transform
│   ├── uppercase/        # ASCII uppercase transform
│   ├── json-parse/       # JSON validation/pretty-print
│   ├── filter/           # Pattern-based filtering
│   ├── content-router/   # Content-based 1→N routing
│   ├── merge-joiner/     # Stateless N→1 merge
│   └── mnist-inference/  # ML inference with WASI-NN
├── examples/             # Example pipeline configurations
├── wit/                  # WIT interface definitions for plugins
├── docs/                 # Documentation
│   ├── api/              # OpenAPI spec and Bruno collection
│   ├── adr/              # Architecture Decision Records
│   └── *.md              # Various docs
└── tests/                # Integration tests
```

## Development Setup

### Using just (Recommended)

```bash
# Show all available commands
just

# Core development workflow
just build              # Build entire workspace
just test               # Run all tests
just check              # Type-check without building
just fmt                # Format code
just clippy             # Run lints

# Build specific components
just build-runtime      # Build wafer-runtime only
just build-ctl          # Build waferctl only
just build-plugins      # Build all WASM plugins
just build-plugin NAME  # Build specific plugin (e.g., just build-plugin uppercase)

# Run pipelines
just run                                    # Run default passthrough pipeline
just run examples/dag-uppercase.toml        # Run specific config
just run-remote                             # Run with OCI-hosted plugins
```

### Manual Commands

```bash
# Build workspace
cargo build --workspace

# Run tests
cargo test --workspace

# Run with verbose logging
RUST_LOG=debug cargo run -p wafer-runtime -- --config examples/dag-passthrough.toml

# Build a plugin
cargo build --release --manifest-path plugins/uppercase/Cargo.toml

# Run waferctl
cargo run -p waferctl -- --help
```

### Running Tests

```bash
# All tests
cargo test --workspace

# Specific crate
cargo test -p wafer-core
cargo test -p wafer-runtime
cargo test -p waferctl

# With feature flags
cargo test -p wafer-core --features http-api

# Integration tests only
cargo test -p wafer-runtime --test integration

# With output
cargo test --workspace -- --nocapture
```

## Running Pipelines

### Basic Usage

```bash
# Runtime binary
cargo run -p wafer-runtime -- --config <path-to-config.toml>

# Or after building
./target/debug/wafer-runtime --config examples/dag-uppercase.toml
```

### Example Pipelines

```bash
# Passthrough (stdin → transform → stdout)
echo "test" | cargo run -p wafer-runtime -- --config examples/dag-passthrough.toml

# Uppercase transform
echo "hello world" | cargo run -p wafer-runtime -- --config examples/dag-uppercase.toml
# Output: HELLO WORLD

# JSON validation and pretty-print
echo '{"key": "value"}' | cargo run -p wafer-runtime -- --config examples/dag-json-parse.toml

# Filter (drops lines matching pattern)
printf "DEBUG: test\nINFO: keep\nDEBUG: drop" | cargo run -p wafer-runtime -- --config examples/dag-filter.toml

# Chained transforms
echo "keep this" | cargo run -p wafer-runtime -- --config examples/dag-chain.toml

# File-based I/O
cargo run -p wafer-runtime -- --config examples/dag-file-io.toml

# Diamond pattern (fan-out/fan-in)
cargo run -p wafer-runtime -- --config examples/dag-diamond.toml

# With control plane API enabled
cargo run -p wafer-runtime -- --config examples/dag-passthrough-with-api.toml
```

### With Control Plane

```bash
# Start pipeline with API
cargo run -p wafer-runtime -- --config examples/dag-passthrough-with-api.toml

# In another terminal, query the API:
curl http://localhost:9090/health
curl http://localhost:9090/api/v1/pipeline
curl http://localhost:9090/api/v1/nodes
curl http://localhost:9091/metrics

# Or use waferctl:
cargo run -p waferctl -- -e http://localhost:9090 status
cargo run -p waferctl -- -e http://localhost:9090 nodes
```

## Building Plugins

Plugins are WebAssembly components that implement the WAFER transform interface.

### Build All Plugins

```bash
just build-plugins
```

### Build Specific Plugin

```bash
just build-plugin uppercase
# Or manually:
cargo build --release --manifest-path plugins/uppercase/Cargo.toml
```

### Create a New Plugin

1. Copy an existing plugin as a template:
   ```bash
   cp -r plugins/pass-through plugins/my-plugin
   ```

2. Update `plugins/my-plugin/Cargo.toml`:
   ```toml
   [package]
   name = "my-plugin"
   
   [lib]
   crate-type = ["cdylib"]
   
   [dependencies]
   wit-bindgen = "0.41"
   ```

3. Implement the transform interface in `src/lib.rs`:
   ```rust
   wit_bindgen::generate!({
       world: "transform",
       path: "../../wit",
   });
   
   struct MyTransform;
   
   impl Guest for MyTransform {
       fn process(input: Vec<u8>) -> Result<Vec<u8>, String> {
           // Your transformation logic here
           Ok(input)
       }
   }
   
   export!(MyTransform);
   ```

4. Build:
   ```bash
   cargo build --release --manifest-path plugins/my-plugin/Cargo.toml
   ```

### Publishing to OCI Registry

```bash
# Login to GitHub Container Registry
just registry-login YOUR_USERNAME YOUR_GITHUB_TOKEN

# Publish a plugin
just publish-plugin uppercase 1.0.0

# Publish all plugins
just publish-all 1.0.0
```

## Control Plane API

When `[api]` is enabled in the config, WAFER exposes a REST API for monitoring and control.

### Endpoints

| Endpoint | Method | Description |
|----------|--------|-------------|
| `/health` | GET | Liveness probe |
| `/ready` | GET | Readiness probe |
| `/api/v1/pipeline` | GET | Pipeline status and metrics |
| `/api/v1/pipeline/drain` | POST | Graceful shutdown |
| `/api/v1/pipeline/shutdown` | POST | Immediate shutdown |
| `/api/v1/nodes` | GET | List all nodes |
| `/api/v1/nodes/{id}` | GET | Get node details |
| `/metrics` | GET | Prometheus metrics |

### Documentation

- **OpenAPI Spec**: [`docs/api/openapi.yaml`](docs/api/openapi.yaml)
- **Bruno Collection**: [`docs/api/bruno-collection/`](docs/api/bruno-collection/) — Import into [Bruno](https://usebruno.com/) for interactive testing

### waferctl CLI

```bash
# Check pipeline status
waferctl -e http://localhost:9090 status

# List nodes
waferctl -e http://localhost:9090 nodes

# Get specific node
waferctl -e http://localhost:9090 node transform

# Drain pipeline (graceful shutdown)
waferctl -e http://localhost:9090 drain

# See all commands
waferctl --help
```

## Configuration Reference

Pipeline configurations are TOML files. See [`examples/`](examples/) for complete examples.

### Basic Structure

```toml
[pipeline]
name = "my-pipeline"
description = "Optional description"

# Control plane (optional)
[api]
enabled = true
bind = "127.0.0.1:9090"

[metrics]
enabled = true
bind = "127.0.0.1:9091"
path = "/metrics"

# Default queue capacity for all edges
default_queue_capacity = 1024

# Nodes
[[nodes]]
id = "source"
node_type = "source"
source_type = "stdin"  # stdin, file, mqtt

[[nodes]]
id = "transform"
node_type = "transform"
swappable = true  # Enable hot-swap
[nodes.config]
plugin_path = "path/to/plugin.wasm"
# Or from OCI registry:
# plugin_ref = "ghcr.io/pedroklein/wafer-uppercase:1.0.0"

[[nodes]]
id = "sink"
node_type = "sink"
sink_type = "stdout"  # stdout, file, mqtt

# Edges define data flow (with optional overflow policy)
[[edges]]
from = "source"
to = "transform"
[edges.queue]
capacity = 1000
overflow = "slow"  # slow (default), drop, or dead-letter

[[edges]]
from = "transform"
to = "sink"
[edges.queue]
overflow = "drop"  # Drop messages when queue is full

# Dead Letter Queue (optional - required if any edge uses dead-letter policy)
[dead_letter]
enabled = true
sink_type = "file"
[dead_letter.config]
path = "/var/log/wafer/dlq.jsonl"
```

### Node Types

| Type | Description | Config |
|------|-------------|--------|
| `source` | Data ingestion | `source_type`: stdin, file, mqtt |
| `transform` | WASM plugin processing | `plugin_path` or `plugin_ref` |
| `router` | Fan-out (1→N) | `plugin_path` + multiple outgoing edges |
| `joiner` | Fan-in (N→1) | `plugin_path` + multiple incoming edges |
| `sink` | Data output | `sink_type`: stdout, file, mqtt; optional `batch_size`, `batch_timeout_ms` |

### Overflow Policies

| Policy | Behavior |
|--------|----------|
| `slow` | Block sender until space available (default, backpressure) |
| `drop` | Discard newest message when queue is full |
| `dead-letter` | Route dropped messages to DLQ for later inspection |

### Environment Variables

| Variable | Description | Default |
|----------|-------------|---------|
| `RUST_LOG` | Log level (error, warn, info, debug, trace) | `info` |
| `WAFER_REGISTRY` | Default OCI registry | `ghcr.io/pedroklein` |
| `WAFER_CACHE_DIR` | Plugin cache directory | `~/.cache/wafer` |

## Documentation

| Document | Description |
|----------|-------------|
| [SPEC.md](docs/SPEC.md) | Complete technical specification |
| [MVP.md](docs/MVP.md) | Current implementation status |
| [api.md](docs/api.md) | Control plane API documentation |
| [REGISTRY.md](docs/REGISTRY.md) | OCI registry integration guide |
| [mqtt-setup.md](docs/mqtt-setup.md) | MQTT source/sink setup |
| [AI_WORKFLOW.md](docs/AI_WORKFLOW.md) | AI-assisted development workflow |
| [ADRs](docs/adr/) | Architecture Decision Records |

## Contributing

This project uses AI-assisted development with [OpenCode](https://agent-runner.ai). 

### Development Workflow

1. Fork and clone the repository
2. Create a feature branch: `git checkout -b feature/my-feature`
3. Make changes and add tests
4. Run checks: `just fmt && just clippy && just test`
5. Commit with conventional commits: `git commit -m "feat: add new feature"`
6. Push and create a pull request

### Code Style

- Follow Rust conventions and `rustfmt` defaults
- Use `clippy` with `-D warnings`
- Add tests for new functionality
- Update documentation as needed

## License

Licensed under either of:

- Apache License, Version 2.0 ([LICENSE-APACHE](LICENSE-APACHE) or http://www.apache.org/licenses/LICENSE-2.0)
- MIT license ([LICENSE-MIT](LICENSE-MIT) or http://opensource.org/licenses/MIT)

at your option.
