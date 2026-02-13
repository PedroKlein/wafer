# Wafer PoC

A Rust-based WebAssembly plugin loader using wasmtime, designed for extensible application architectures.

## Features

- **wasmtime 40.x runtime** - Production-ready WASM execution
- **WASI support** - System interface for plugins (filesystem, environment, clocks)
- **Async execution** - Tokio-based async runtime integration
- **MQTT integration** - Event-driven plugin communication via rumqttc

## Quick Start

```bash
# Build the project
cargo build

# Run in release mode
cargo run --release
```

## Project Structure

```
wafer-poc/
├── src/              # Core plugin loader implementation
├── plugins/          # WASM plugin modules
├── .opencode/        # AI agent configurations
│   └── agents/       # Specialist subagents
└── Cargo.toml        # Dependencies and build config
```

---

## Vibe Coding Workflow

This project uses [OpenCode](https://opencode.ai) with specialized AI agents and the [Beads](https://github.com/steveyegge/beads) task tracking system for an optimized AI-assisted development workflow.

### Prerequisites

```bash
# Install beads CLI
brew install beads
# OR
curl -fsSL https://raw.githubusercontent.com/steveyegge/beads/main/scripts/install.sh | bash

# Initialize beads in the project
bd init
```

### Workflow Overview

```
┌─────────────────────────────────────────────────────────────────┐
│                     VIBE CODING LOOP                            │
├─────────────────────────────────────────────────────────────────┤
│                                                                 │
│   1. DESCRIBE  ──►  2. DECOMPOSE  ──►  3. EXECUTE  ──►  4. VERIFY  │
│        │                 │                  │               │   │
│   "Add caching"    @orchestrator      @specialists      tests   │
│                    breaks it down     do the work       pass    │
│                                                                 │
└─────────────────────────────────────────────────────────────────┘
```

### Step-by-Step

1. **Describe** what you want to build in natural language
2. **Decompose** with `@orchestrator` to break complex features into tasks
3. **Execute** tasks with specialist agents or `@beads-task-agent`
4. **Verify** changes work, then repeat

### Using Beads Tasks

```bash
# Create a task
bd create "Implement plugin caching" -p 1

# See what's ready to work on
bd ready

# Claim a task before starting
bd update <id> --claim

# Add notes as you work
bd update <id> --notes "Using LRU with 100MB limit"

# Complete the task
bd close <id> --reason "Implemented with SHA256-based cache keys"

# Sync to git
bd sync
```

### Agent Capabilities

| Agent                  | Purpose                             | Can Modify Code? |
| ---------------------- | ----------------------------------- | ---------------- |
| `@orchestrator`        | Task decomposition & coordination   | Yes              |
| `@beads-task-agent`    | Autonomous task execution           | Yes              |
| `@wasm-specialist`     | Wasmtime runtime & plugin arch      | Yes              |
| `@cargo-expert`        | Dependencies & build configuration  | Yes              |
| `@benchmarker`         | Performance analysis & optimization | Yes              |
| `@rust-analyzer`       | Code audits & safety reviews        | No (read-only)   |
| `@rust-best-practices` | Idiomatic Rust pattern reviews      | No (read-only)   |
| `@rust-teacher`        | Concept explanations & teaching     | No (read-only)   |

### Example Workflows

#### Adding a New Feature

```
You: Add hot-reload support for plugins

@orchestrator: Breaking this down into tasks...
  1. Design file watcher integration
  2. Implement module reload mechanism  
  3. Handle in-flight requests during reload
  4. Add integration tests

[Creates beads tasks with dependencies]

You: @beads-task-agent work on ready tasks

[Agent claims and completes tasks autonomously]
```

#### Performance Investigation

```
You: Plugin loading feels slow

@benchmarker: Let me measure the current state...
  - Cold start: 45ms
  - Warm start: 2ms
  - Bottleneck: Module compilation

@orchestrator: Creating optimization tasks...
  1. Implement module caching
  2. Add precompilation on startup
  3. Measure improvements
```

#### Code Review

```
You: Review the plugin loader for safety issues

@rust-analyzer: Analyzing src/loader.rs...
  
  Critical Issues:
  - Line 47: unwrap() on user input
  - Line 89: potential race condition
  
  Recommendations:
  - Add timeout for plugin initialization
  - Use parking_lot for better mutex performance
```

### Key Commands

| Command                       | Description                     |
| ----------------------------- | ------------------------------- |
| `bd ready`                    | List unblocked tasks            |
| `bd create "title" -p <0-3>`  | Create task (0=critical, 3=low) |
| `bd update <id> --claim`      | Claim a task                    |
| `bd update <id> --notes "x"`  | Add notes                       |
| `bd close <id> --reason "x"`  | Complete task                   |
| `bd dep add <child> <parent>` | Add dependency                  |
| `bd sync`                     | Commit task changes to git      |
| `bd status`                   | Show all tasks                  |

### Tips for Effective Vibe Coding

1. **Start broad, refine narrow** - Let `@orchestrator` break down big ideas
2. **Use read-only agents for reviews** - `@rust-analyzer` and `@rust-best-practices` won't accidentally change code
3. **Trust `bd ready`** - It ensures you work on unblocked tasks
4. **Add notes liberally** - Future you (and agents) will thank you
5. **Sync frequently** - `bd sync` persists task state in git

---

## License

MIT
