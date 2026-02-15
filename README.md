# Wafer PoC

**WAFER** - WebAssembly Flow Execution Runtime

A Rust-based DAG pipeline runtime that executes WebAssembly plugins using wasmtime, with support for hot-swapping, bounded queues, and fuel-based execution metering.

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

# Run tests (includes DAG integration tests)
cargo test

# Run single-transform pipeline
echo "hello" | cargo run -- --config pipeline.toml

# Using just (if installed)
just build        # Build the project
just test         # Run tests
just plugin       # Build the plugin
just run          # Run with pass-through plugin
```

### DAG Pipelines

Multi-node DAG pipelines are available via the `DagOrchestrator` API:

```rust
// FileSource → Transform → FileSink
let config: DagConfig = toml::from_str(config_toml)?;
let mut orchestrator = DagOrchestrator::from_config(config)?;
orchestrator.register_node("source", AnyNode::from_source(FileSource::new("source", "in.txt")))?;
orchestrator.wire_queues()?;
orchestrator.run().await?;
```

See [MVP.md](docs/MVP.md) for detailed usage and [examples/dag-config.toml](examples/dag-config.toml) for configuration format.

## Project Structure

```
wafer-poc/
├── src/
│   ├── dag/               # DAG orchestrator with petgraph
│   ├── engine/            # Wasmtime engine and WASI bindings
│   ├── node/              # Source, Transform, Sink traits and impls
│   ├── pipeline/          # Single-transform pipeline executor
│   ├── queue/             # Bounded SPSC queues
│   └── config/            # TOML configuration loading
├── wit/                    # WIT interface definitions
├── plugins/                # Example WASM plugins
├── tests/
│   ├── integration.rs     # DAG pipeline integration tests
│   └── fixtures/          # Test TOML configs
├── examples/
│   └── dag-config.toml    # Example DAG pipeline config
└── docs/
    ├── SPEC.md            # Technical specification
    ├── MVP.md             # Current implementation status
    ├── AI_WORKFLOW.md     # AI development workflow
    └── adr/               # Architecture decisions
```

## Key Features

- **Multi-node DAG pipelines** with linear chain execution
- **Wasmtime 41.x** runtime with async support
- **SPSC bounded queues** for inter-node communication with backpressure
- **petgraph-based topology** for DAG management
- **FileSource/FileSink** for file-based I/O
- **Fuel-based metering** for execution limits
- **WASI Preview 2** for plugin capabilities
- **Graceful shutdown** in reverse topological order

## Contributing

This project uses AI-assisted development with [OpenCode](https://agent-runner.ai). See [AI_WORKFLOW.md](docs/AI_WORKFLOW.md) for the development workflow, agent capabilities, and task management with Beads.

## License

MIT
