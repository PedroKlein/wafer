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

# Run tests
cargo test

# Run with a plugin
cargo run -- --plugin plugins/example.wasm

# Using just (if installed)
just build        # Build the project
just test         # Run tests
just plugin       # Build the plugin
just run          # Run with pass-through plugin
```

## Project Structure

```
wafer-poc/
├── src/                    # Runtime implementation
├── wit/                    # WIT interface definitions
├── plugins/                # Example WASM plugins
├── docs/
│   ├── SPEC.md            # Technical specification
│   ├── AI_WORKFLOW.md     # AI development workflow
│   └── adr/               # Architecture decisions
└── .tasks/agents/      # AI agent configurations
```

## Key Features

- **Wasmtime 41.x** runtime with async support
- **SPSC bounded queues** for node communication
- **Drain-and-flip hot-swap** for zero-downtime updates
- **Fuel-based metering** for execution limits
- **WASI Preview 2** for plugin capabilities

## Contributing

This project uses AI-assisted development with [OpenCode](https://agent-runner.ai). See [AI_WORKFLOW.md](docs/AI_WORKFLOW.md) for the development workflow, agent capabilities, and task management with Beads.

## License

MIT
