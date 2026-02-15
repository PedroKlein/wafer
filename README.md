# Wafer PoC

**WAFER** - WebAssembly Flow Execution Runtime

A Rust-based DAG pipeline runtime that executes WebAssembly plugins using wasmtime, with support for bounded queues and fuel-based execution metering.

## Documentation

| Document                              | Description                                |
| ------------------------------------- | ------------------------------------------ |
| [SPEC.md](docs/SPEC.md)               | Complete technical specification           |
| [AI_WORKFLOW.md](docs/AI_WORKFLOW.md) | AI-assisted development guide with OpenCode |
| [MVP.md](docs/MVP.md)                 | Current MVP state and module structure     |
| [ADRs](docs/adr/)                     | Architecture Decision Records              |

## Quick Start

```bash
# Build the project
cargo build

# Run tests
cargo test

# Run DAG pipeline with stdin/stdout
echo "hello" | cargo run -- --config examples/dag-passthrough.toml

# Run with uppercase transform
echo "hello" | cargo run -- --config examples/dag-uppercase.toml
# Output: HELLO

# Using just (if installed)
just build        # Build the project
just test         # Run tests
just plugin       # Build the plugin
just run          # Run with pass-through plugin
```

### Example DAG Configs

```bash
# Passthrough (stdin → transform → stdout)
echo "test" | cargo run -- --config examples/dag-passthrough.toml

# Uppercase transform
echo "hello world" | cargo run -- --config examples/dag-uppercase.toml

# JSON validation and pretty-print
echo '{"key": "value"}' | cargo run -- --config examples/dag-json-parse.toml

# Filter (drops lines matching pattern)
printf "DEBUG: test\nINFO: keep\nDEBUG: drop" | cargo run -- --config examples/dag-filter.toml

# Chained transforms (uppercase → filter)
echo "keep this" | cargo run -- --config examples/dag-chain.toml

# File-based I/O
cargo run -- --config examples/dag-file-io.toml
```

## Project Structure

```
wafer-poc/
├── src/
│   ├── dag/               # DAG orchestrator with petgraph
│   ├── engine/            # Wasmtime engine and WASI bindings
│   ├── node/              # Source, Transform, Sink traits and impls
│   ├── queue/             # Bounded SPSC queues
│   └── config/            # TOML configuration loading
├── wit/                    # WIT interface definitions
├── plugins/                # WASM transform plugins
│   ├── pass-through/      # No-op passthrough
│   ├── uppercase/         # ASCII uppercase
│   ├── json-parse/        # JSON validation/pretty-print
│   └── filter/            # Pattern-based filtering
├── tests/
│   ├── integration.rs     # DAG pipeline integration tests
│   └── fixtures/          # Test TOML configs
├── examples/              # Example DAG pipeline configs
│   ├── dag-passthrough.toml
│   ├── dag-uppercase.toml
│   ├── dag-json-parse.toml
│   ├── dag-filter.toml
│   ├── dag-chain.toml
│   └── dag-file-io.toml
└── docs/
    ├── SPEC.md            # Technical specification
    ├── MVP.md             # Current implementation status
    ├── AI_WORKFLOW.md     # AI development workflow
    └── adr/               # Architecture decisions
```

## Key Features

- **DAG-only architecture** - All pipelines defined as directed acyclic graphs
- **StdinSource/StdoutSink** - First-class stdin/stdout I/O for CLI usage
- **FileSource/FileSink** - File-based I/O for batch processing
- **Wasmtime 41.x** runtime with async support
- **SPSC bounded queues** for inter-node communication with backpressure
- **petgraph-based topology** for DAG management
- **Fuel-based metering** for execution limits
- **WASI Preview 2** for plugin capabilities
- **Graceful shutdown** in reverse topological order

## DAG Configuration Format

```toml
[[nodes]]
id = "source"
node_type = "source"
source_type = "stdin"  # or "file" with config.path

[[nodes]]
id = "transform"
node_type = "transform"
config = { plugin_path = "path/to/plugin.wasm" }

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

## Contributing

This project uses AI-assisted development with [OpenCode](https://opencode.ai). See [AI_WORKFLOW.md](docs/AI_WORKFLOW.md) for the development workflow.

## License

MIT
