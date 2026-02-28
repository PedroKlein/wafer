# Agent Definitions

This document defines the specialized agents available for the WAFER project.

## Available Agents

### @orchestrator
**Purpose:** Task decomposition & coordination

Coordinates complex work by breaking features into tasks, managing dependencies, and dispatching to specialist agents. Maintains spec/ADR consistency.

**Responsibilities:**
- Break down large features into beads tasks
- Ensure alignment with SPEC.md
- Create ADR tasks when architectural decisions needed
- Coordinate between specialist agents

### @task-id
**Purpose:** Autonomous task execution

Finds ready tasks via `bd ready`, claims them, and completes work independently. Ideal for batch processing multiple tasks.

**Responsibilities:**
- Execute ready tasks from the beads queue
- Update task status as work progresses
- Create follow-up tasks as needed

### @wasm-specialist
**Purpose:** Wasmtime runtime & plugin architecture

Deep expertise in wasmtime runtime, plugin architecture, WASI integration, and the Component Model. Primary agent for core implementation work.

**Domain Knowledge:**
- Wasmtime component model and WASI P2
- WIT contracts (types.wit, lifecycle.wit, transform.wit, router.wit, joiner.wit)
- Fuel/epoch metering
- WASI capability injection
- wasi-nn integration

**SPEC Reference:** Sections 3, 4, 11

### @cargo-expert
**Purpose:** Dependencies & build configuration

Handles Cargo.toml configuration, workspace setup, build optimization, and feature flags. Knows wasmtime-specific feature requirements.

**Domain Knowledge:**
- Wasmtime dependency features
- wasm32-wasip2 target configuration
- Cross-compilation for ARM/x86
- wit-bindgen setup

### @benchmarker
**Purpose:** Performance analysis & optimization

Measures, analyzes, and recommends optimizations using criterion, hyperfine, and profiling tools.

**Domain Knowledge:**
- SPEC Section 15 (Evaluation Plan)
- Baseline comparisons (native, out-of-process, eKuiper)
- Platform-specific benchmarks (Pi, Mac M3, x86, Jetson)
- Hot-swap timing metrics

### @rust-analyzer
**Purpose:** Code audits & safety reviews (read-only)

Focuses on ownership, concurrency, error handling, and safety. Provides findings without making changes.

**Focus Areas:**
- Async safety (Send/Sync bounds)
- Error propagation patterns
- Memory safety in WASM host integration

### @rust-best-practices
**Purpose:** Idiomatic Rust pattern reviews (read-only)

Reviews code for idiomatic patterns, API guidelines compliance, and documentation quality.

**Focus Areas:**
- Rust 2024 conventions
- API ergonomics
- Documentation completeness

### @rust-teacher
**Purpose:** Concept explanations & teaching

Educational agent that explains Rust, WASM, WIT, and Component Model concepts.

**Topics:**
- WASI Preview 2 concepts
- Component Model architecture
- WIT interface design
- Async Rust patterns

## Agent Selection Guide

| Task Type | Primary Agent | Support Agents |
|-----------|---------------|----------------|
| New feature implementation | @orchestrator → @wasm-specialist | @cargo-expert |
| Performance investigation | @benchmarker | @rust-analyzer |
| Bug fix | @wasm-specialist | @rust-analyzer |
| Code review | @rust-analyzer + @rust-best-practices | - |
| Architectural decision | @orchestrator | All specialists |
| Learning/onboarding | @rust-teacher | - |
| Bulk task execution | @task-id | - |

## SPEC Domain Mapping

| SPEC Section | Primary Agent |
|--------------|---------------|
| §3 Architecture | @wasm-specialist |
| §4 WIT Contracts | @wasm-specialist |
| §5 Node Categories | @wasm-specialist |
| §8 Queue/Backpressure | @wasm-specialist |
| §10 Hot-Swap | @wasm-specialist, @benchmarker |
| §11 Inference | @wasm-specialist |
| §12 Observability | @wasm-specialist |
| §15 Evaluation | @benchmarker |
| §16 Comparisons | @benchmarker |

## ADR Workflow Integration

When an architectural decision is needed:

1. `@orchestrator` creates ADR task: `bd create "ADR: <topic>" -p 1 -l adr,decision`
2. Specialist agents research options, add notes to task
3. Write ADR in `docs/adr/NNNN-<slug>.md` with status: Proposed
4. User reviews and accepts/rejects
5. On acceptance, create implementation tasks

See [docs/adr/README.md](../docs/adr/README.md) for ADR template.
