# AI-Assisted Development Workflow

This project uses [OpenCode](https://agent-runner.ai) with specialized AI agents and the [Beads](https://github.com/steveyegge/beads) task tracking system for an optimized AI-assisted development workflow.

## Prerequisites

```bash
# Install beads CLI
brew install beads
# OR
curl -fsSL https://raw.githubusercontent.com/steveyegge/beads/main/scripts/install.sh | bash

# Initialize beads in the project
bd init
```

## Workflow Overview

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
3. **Execute** tasks with specialist agents or `@task-id`
4. **Verify** changes work, then repeat

---

## Using tasks Tasks

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

---

## Agent Capabilities

| Agent                  | Purpose                             | Can Modify Code? |
| ---------------------- | ----------------------------------- | ---------------- |
| `@orchestrator`        | Task decomposition & coordination   | Yes              |
| `@task-id`    | Autonomous task execution           | Yes              |
| `@wasm-specialist`     | Wasmtime runtime & plugin arch      | Yes              |
| `@cargo-expert`        | Dependencies & build configuration  | Yes              |
| `@benchmarker`         | Performance analysis & optimization | Yes              |
| `@rust-analyzer`       | Code audits & safety reviews        | No (read-only)   |
| `@rust-best-practices` | Idiomatic Rust pattern reviews      | No (read-only)   |
| `@rust-teacher`        | Concept explanations & teaching     | No (read-only)   |

### Agent Descriptions

**Orchestrator (`@orchestrator`)**
Coordinates complex work by breaking features into tasks, managing dependencies, and dispatching to specialist agents. Also maintains spec/ADR consistency.

**Task Agent (`@task-id`)**
Autonomous executor that finds ready tasks, claims them, and completes work independently. Ideal for batch processing multiple tasks.

**WASM Specialist (`@wasm-specialist`)**
Deep expertise in wasmtime runtime, plugin architecture, WASI integration, and the Component Model. Go-to for implementation work.

**Cargo Expert (`@cargo-expert`)**
Handles dependencies, Cargo.toml configuration, workspace setup, and build optimization. Knows wasmtime feature flags.

**Benchmarker (`@benchmarker`)**
Performance analysis using criterion, hyperfine, and profiling tools. Measures, analyzes, and recommends optimizations.

**Rust Analyzer (`@rust-analyzer`)**
Read-only code auditor focusing on ownership, concurrency, error handling, and safety. Provides findings without making changes.

**Rust Best Practices (`@rust-best-practices`)**
Reviews code for idiomatic Rust patterns, API guidelines compliance, and documentation quality. Read-only.

**Rust Teacher (`@rust-teacher`)**
Educational agent that explains Rust, WASM, WIT, and Component Model concepts. Great for learning and onboarding.

---

## Example Workflows

### Adding a New Feature

```
You: Add hot-reload support for plugins

@orchestrator: Breaking this down into tasks...
  1. Design file watcher integration
  2. Implement module reload mechanism  
  3. Handle in-flight requests during reload
  4. Add integration tests

[Creates beads tasks with dependencies]

You: @task-id work on ready tasks

[Agent claims and completes tasks autonomously]
```

### Performance Investigation

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

### Code Review

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

---

## Decision Workflow (ADRs)

Architectural decisions are tracked in `docs/adr/` using Architecture Decision Records. This integrates with beads for a structured decision-making process.

### Creating an ADR Task

```bash
# When you identify a decision point
bd create "ADR: Choose queue implementation" -p 1 -l adr,decision

# Find pending ADR tasks
bd query "label=adr"
```

### ADR Workflow

```
┌──────────────────────────────────────────────────────────────────┐
│                     ADR DECISION FLOW                            │
├──────────────────────────────────────────────────────────────────┤
│                                                                  │
│   1. IDENTIFY  ──►  2. RESEARCH  ──►  3. PROPOSE  ──►  4. DECIDE │
│        │                │                 │                │     │
│   Create ADR task   @specialists      Write ADR         Accept/  │
│   with beads        investigate       as "Proposed"     Reject   │
│                                                                  │
└──────────────────────────────────────────────────────────────────┘
```

1. **Identify**: Agent or user creates ADR task when architectural choice needed
2. **Research**: Specialist agents investigate options, add findings to task notes
3. **Propose**: Write ADR in `docs/adr/NNNN-title.md` with status "Proposed"
4. **Decide**: User accepts or requests changes; update status to "Accepted"

### Example

```bash
# 1. Create the task
bd create "ADR: SPSC vs MPSC queues for node communication" -p 1 -l adr,decision

# 2. Claim and research
bd update abc123 --claim
bd update abc123 --notes "SPSC: lower latency, simpler. MPSC: more flexible fan-in"

# 3. Write proposed ADR
# (agent creates docs/adr/0004-queue-type.md with status: proposed)

# 4. After user approval
bd close abc123 --reason "Accepted: SPSC chosen for latency requirements"
bd sync
```

See [ADR README](adr/README.md) for the full template.

---

## Spec Reference

The authoritative source of truth for this project is [`docs/SPEC.md`](SPEC.md).

All agents are configured to:
- Reference relevant SPEC sections for their domain
- Flag conflicts or ambiguities to `@orchestrator`
- Create ADR tasks when architectural decisions are needed

---

## Tips for Effective AI-Assisted Development

1. **Start broad, refine narrow** - Let `@orchestrator` break down big ideas
2. **Use read-only agents for reviews** - `@rust-analyzer` and `@rust-best-practices` won't accidentally change code
3. **Trust `bd ready`** - It ensures you work on unblocked tasks
4. **Add notes liberally** - Future you (and agents) will thank you
5. **Sync frequently** - `bd sync` persists task state in git
6. **Document decisions** - Use ADRs for architectural choices
7. **Let agents specialize** - Use the right agent for the job
8. **Verify before moving on** - Run tests after each task completion
