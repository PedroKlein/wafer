# MVP Vertical Slice: WAFER Pipeline Runtime

## TL;DR

> **Quick Summary**: Build a minimal end-to-end WAFER pipeline demonstrating stdin → pass-through.wasm → stdout. Uses wasmtime Component Model with WIT contracts, SPSC bounded queues, and fuel metering.
> 
> **Deliverables**:
> - Fixed Cargo.toml with correct edition and dependencies
> - Minimal WIT contracts (types + transform + lifecycle)
> - Wasmtime component loader with WASI P2 support
> - SPSC bounded queue wrapper
> - Pass-through transform WASM plugin
> - Pipeline executor with TOML config
> - Basic metrics (3 counters/gauges)
> 
> **Estimated Effort**: Medium (1-2 weeks)
> **Parallel Execution**: YES - 3 waves
> **Critical Path**: Task 1 → Task 2 → Task 3 → Task 5 → Task 6 → Task 8

---

## Context

### Original Request
Build an MVP "vertical slice" of WAFER - a Rust-based DAG pipeline runtime that executes WebAssembly plugins using wasmtime. The MVP should demonstrate a working end-to-end flow: stdin → pass-through.wasm → stdout.

### Interview Summary
**Key Discussions**:
- **Performance priority**: Performance from start (target p50 <10ms at 100 msg/s)
- **WASM model**: Component Model with WIT contracts (not core wasm modules)
- **Node categories**: Source + Transform + Sink only (no Router/Joiner)
- **I/O approach**: Mock/Stdin first, defer MQTT to later milestone
- **Test strategy**: Tests-after (not TDD)
- **Example plugin**: Yes, include pass-through transform
- **Implementation order**: Vertical slice (minimal end-to-end first)

**Research Findings**:
- Project is greenfield - only `println!("Hello, world!")` exists
- Cargo.toml has edition "2024" (invalid - must be "2021")
- Missing dependencies: crossbeam-channel, serde, toml, tracing, wit-bindgen
- wasmtime 41.x has stable async component instantiation
- WASI P2 requires `WasiView` trait with `WasiCtx` + `ResourceTable`
- 3 ADRs accepted: wasmtime selection, SPSC queues, drain-and-flip

### Metis Review
**Identified Gaps** (addressed):
- **WIT versioning**: All packages share 0.1.0 for MVP simplicity
- **Payload subset**: MVP uses only `payload::raw(list<u8>)`, not all variants
- **Lifecycle subset**: MVP uses `init()` + `process()` only, defers `validate()` + `close()`
- **Backpressure**: MVP uses blocking (slow) policy only
- **Error handling**: Log and drop on transform error
- **Message size**: 1MB limit documented but not enforced in MVP

---

## Rust Best Practices (CRITICAL - Project Structure)

> **User Requirement**: "I want the project structure to be very good on modules. This is the most important part."
>
> This section defines idiomatic Rust patterns to follow, based on research of production projects:
> **wasmtime**, **tokio**, **vector**, **spin** (all 10k+ GitHub stars, actively maintained).

### Reference Projects Studied

| Project | Domain | Key Pattern to Copy |
|---------|--------|---------------------|
| **wasmtime** | WASM runtime | `pub(crate) mod`, Engine/Store/Linker abstraction, thiserror |
| **tokio** | Async runtime | Module organization (scheduler/, task/, io/), feature flags |
| **vector** | Data pipeline | Topology builder pattern, separate crates for core vs plugins |
| **spin** | WASM microservices | Factor trait for plugin systems, `crates/` workspace |

### Project Structure to Follow

```
wafer-poc/
├── Cargo.toml                    # Workspace root
├── crates/                       # Internal crates (for future expansion)
│   └── (empty for MVP)
├── src/
│   ├── lib.rs                    # Public API re-exports
│   ├── main.rs                   # CLI entry point only
│   ├── error.rs                  # Centralized error types (thiserror)
│   ├── engine/
│   │   ├── mod.rs                # pub use re-exports
│   │   ├── loader.rs             # pub(crate) - WaferEngine
│   │   ├── host.rs               # pub(crate) - WasiView impl
│   │   └── instance.rs           # pub(crate) - TransformInstance
│   ├── queue/
│   │   ├── mod.rs
│   │   ├── bounded.rs            # pub(crate)
│   │   └── envelope.rs           # pub(crate)
│   ├── pipeline/
│   │   ├── mod.rs
│   │   ├── executor.rs           # pub(crate)
│   │   └── builder.rs            # pub(crate)
│   ├── config/
│   │   ├── mod.rs
│   │   ├── schema.rs             # pub - config structs (serde)
│   │   └── loader.rs             # pub(crate)
│   └── metrics/
│       ├── mod.rs
│       └── counters.rs           # pub(crate)
├── plugins/
│   └── pass-through/             # Example plugin (separate crate)
├── wit/                          # WIT contracts
├── examples/
│   └── pass-through.toml
└── tests/
    └── integration.rs
```

### Module Visibility Rules (MANDATORY)

Follow the **tokio/wasmtime pattern**: implementation in `pub(crate)` modules, public API via re-exports.

```rust
// src/engine/mod.rs - PUBLIC re-exports only
mod loader;
mod host;
mod instance;

// Re-export only what external code needs
pub use loader::WaferEngine;
pub use instance::TransformInstance;

// Internal items stay pub(crate)
pub(crate) use host::WaferState;
```

```rust
// src/engine/loader.rs - INTERNAL implementation
pub(crate) struct WaferEngine {
    engine: wasmtime::Engine,
    // fields are private
}

impl WaferEngine {
    pub fn new() -> Result<Self> { ... }  // pub = visible via re-export
    pub(crate) fn inner(&self) -> &wasmtime::Engine { ... }  // crate-internal only
}
```

**Decision Table:**

| Visibility | Use When |
|------------|----------|
| `pub` | Types/functions in public API (re-exported from mod.rs) |
| `pub(crate)` | Shared across modules but not part of public API |
| `pub(super)` | Helper functions for parent module only |
| (private) | Single-module implementation details |

### Error Handling Rules (MANDATORY)

**Pattern**: Use `thiserror` for all error types. This is a library/runtime, not just an application.

```rust
// src/error.rs - Centralized error types
use thiserror::Error;

#[derive(Error, Debug)]
pub enum WaferError {
    #[error("failed to load component from {path}: {source}")]
    ComponentLoad {
        path: std::path::PathBuf,
        #[source]
        source: wasmtime::Error,
    },
    
    #[error("plugin initialization failed: {message}")]
    PluginInit { message: String },
    
    #[error("process() returned error: code={code}, message={message}")]
    ProcessError { code: u32, message: String },
    
    #[error("configuration error: {0}")]
    Config(#[from] ConfigError),
    
    #[error("queue full after timeout")]
    QueueFull,
    
    #[error(transparent)]
    Io(#[from] std::io::Error),
}

#[derive(Error, Debug)]
pub enum ConfigError {
    #[error("failed to read config file: {0}")]
    Read(#[source] std::io::Error),
    
    #[error("failed to parse TOML: {0}")]
    Parse(#[source] toml::de::Error),
    
    #[error("plugin path does not exist: {0}")]
    PluginNotFound(std::path::PathBuf),
}

pub type Result<T> = std::result::Result<T, WaferError>;
```

**Error Rules:**
- ✅ Always use `#[error("lowercase message")]` (no trailing punctuation)
- ✅ Use `#[from]` for automatic `From` impl where sensible
- ✅ Use `#[source]` to chain underlying errors
- ✅ Include context in error message (path, code, etc.)
- ❌ Never use `()` as error type
- ❌ Never use `anyhow` in library code (OK in main.rs/tests)

### Naming Conventions (MANDATORY - RFC 430)

| Item | Convention | Example |
|------|------------|---------|
| Modules | `snake_case` | `engine`, `queue`, `pipeline` |
| Types/Structs | `UpperCamelCase` | `WaferEngine`, `BoundedQueue` |
| Traits | `UpperCamelCase` | `Transform`, `Lifecycle` |
| Functions | `snake_case` | `load_component`, `process_envelope` |
| Constants | `SCREAMING_SNAKE_CASE` | `DEFAULT_FUEL_LIMIT` |
| Constructors | `new` or `with_*` | `WaferEngine::new()`, `Config::with_defaults()` |

**Conversion Methods:**
| Prefix | Semantics |
|--------|-----------|
| `as_*` | Cheap borrow → borrow |
| `to_*` | Expensive borrow → owned |
| `into_*` | Consumes self → owned |

**Getters:** Use `field()` not `get_field()`:
```rust
impl Config {
    pub fn fuel_limit(&self) -> u64 { self.fuel_limit }
    pub fn fuel_limit_mut(&mut self) -> &mut u64 { &mut self.fuel_limit }
}
```

### Struct Design Rules

**Private fields with getters** (from rust-lang/api-guidelines C-STRUCT-PRIVATE):
```rust
// ✅ CORRECT: Private fields, public getters
pub struct PipelineConfig {
    name: String,
    transform: TransformConfig,
}

impl PipelineConfig {
    pub fn name(&self) -> &str { &self.name }
    pub fn transform(&self) -> &TransformConfig { &self.transform }
}

// ❌ WRONG: Public fields lock you into representation
pub struct PipelineConfig {
    pub name: String,  // Can never change to Cow<str> later
}
```

**Exception**: Config structs with `#[derive(Deserialize)]` may have public fields if they're purely data transfer.

### Common Traits to Implement (MANDATORY)

All public types MUST implement these traits where applicable:

| Trait | When to Implement |
|-------|------------------|
| `Debug` | ALWAYS (use `#[derive(Debug)]`) |
| `Clone` | If type can be meaningfully cloned |
| `Default` | If there's a sensible default |
| `Send + Sync` | Automatic unless using raw pointers - verify your types are `Send + Sync` |

```rust
#[derive(Debug, Clone)]
pub struct TransformConfig {
    name: String,
    plugin_path: PathBuf,
    fuel_limit: u64,
}

impl Default for TransformConfig {
    fn default() -> Self {
        Self {
            name: String::new(),
            plugin_path: PathBuf::new(),
            fuel_limit: DEFAULT_FUEL_LIMIT,
        }
    }
}
```

### Builder Pattern (for complex configuration)

Follow wasmtime's `Config` builder pattern:
```rust
pub struct EngineConfig {
    fuel_limit: u64,
    async_support: bool,
}

impl EngineConfig {
    pub fn new() -> Self {
        Self::default()
    }
    
    /// Set fuel limit per process() call. Returns `&mut Self` for chaining.
    pub fn fuel_limit(&mut self, limit: u64) -> &mut Self {
        self.fuel_limit = limit;
        self
    }
    
    /// Enable async support. Returns `&mut Self` for chaining.
    pub fn async_support(&mut self, enable: bool) -> &mut Self {
        self.async_support = enable;
        self
    }
}

// Usage: EngineConfig::new().fuel_limit(500_000).async_support(true)
```

### Workspace Dependencies (MANDATORY)

All dependencies MUST be declared in workspace root even if used by single crate:
```toml
# Cargo.toml (workspace root)
[workspace]
members = ["plugins/*"]
resolver = "2"

[workspace.dependencies]
wasmtime = "41.0.3"
wasmtime-wasi = "41.0.3"
crossbeam-channel = "0.5"
serde = { version = "1", features = ["derive"] }
toml = "0.8"
tracing = "0.1"
tracing-subscriber = { version = "0.3", features = ["env-filter"] }
thiserror = "2"
tokio = { version = "1", features = ["rt", "io-std", "macros"] }

[package]
name = "wafer"
edition = "2021"

[dependencies]
wasmtime = { workspace = true }
wasmtime-wasi = { workspace = true }
# ... etc
```

### Module Documentation (MANDATORY)

Every module MUST have a doc comment explaining its purpose:
```rust
//! Engine module - Wasmtime component loading and execution.
//!
//! This module provides the core runtime abstractions:
//! - [`WaferEngine`] - Configured wasmtime Engine with fuel metering
//! - [`TransformInstance`] - Instantiated transform component
//!
//! # Example
//! ```ignore
//! let engine = WaferEngine::new()?;
//! let component = engine.load_component("plugin.wasm")?;
//! ```

mod loader;
mod host;
mod instance;

pub use loader::WaferEngine;
pub use instance::TransformInstance;
```

---

## Work Objectives

### Core Objective
Demonstrate a working vertical slice: stdin → pass-through WASM transform → stdout, proving the Component Model + WIT + wasmtime architecture works end-to-end.

### Concrete Deliverables
- `Cargo.toml` with correct edition and all dependencies
- `wit/` directory with minimal WIT contracts
- `src/engine/` - Wasmtime component loader
- `src/queue/` - SPSC bounded queue wrapper
- `src/pipeline/` - Pipeline executor
- `src/config/` - TOML configuration parser
- `src/metrics/` - Basic metrics (3 metrics only)
- `plugins/pass-through/` - Example WASM plugin (Rust source + build)
- `examples/pass-through.toml` - Example pipeline config

### Definition of Done
- [ ] `echo "test" | cargo run -- --config examples/pass-through.toml` outputs "test"
- [ ] Plugin loads without error, logs "Pipeline started"
- [ ] Queue backpressure blocks sender when full (test with small queue)
- [ ] Fuel metering catches runaway execution (test with infinite loop plugin)
- [ ] All cargo clippy warnings addressed
- [ ] Basic metrics visible in logs (messages_total, process_time_ns, queue_depth)

### Must Have
- WIT contract files in `wit/` directory
- Wasmtime Engine + Store + WasiView pattern
- SPSC bounded queue with configurable capacity
- TOML config for pipeline definition
- Fuel metering enabled (default: 1_000_000 fuel units)
- Pass-through transform plugin that compiles to WASM component

### Must NOT Have (Guardrails)
- **NO Router/Joiner nodes** - MVP is linear pipeline only
- **NO MQTT integration** - Defer to M1
- **NO hot-swap mechanism** - Defer to M1
- **NO dynamic topology** - Static config at startup only
- **NO REST API** - Config file only
- **NO config file watching** - Single load at startup
- **NO payload variants beyond raw** - Only `payload::raw(list<u8>)`
- **NO validate() or close()** - Only `init()` + `process()` in lifecycle
- **NO more than 3 metrics** - messages_total, process_time_ns, queue_depth only
- **NO parallel workers per node** - Single-threaded per node
- **NO multi-pipeline management** - Single pipeline only

---

## Verification Strategy (MANDATORY)

> **UNIVERSAL RULE: ZERO HUMAN INTERVENTION**
>
> ALL tasks in this plan MUST be verifiable WITHOUT any human action.
> This is NOT conditional - it applies to EVERY task.
>
> **FORBIDDEN** - acceptance criteria that require:
> - "User manually tests..." / "User visually confirms..."
> - "Ask user to verify..." / ANY step where a human must act
>
> **ALL verification is executed by the agent** using tools (Bash, interactive_bash).

### Test Decision
- **Infrastructure exists**: NO (greenfield)
- **Automated tests**: Tests-after (not TDD)
- **Framework**: `cargo test` (built-in)

### Agent-Executed QA Scenarios (MANDATORY - ALL tasks)

Every task includes concrete verification scenarios the agent executes directly.

**Verification Tool by Deliverable Type:**

| Type | Tool | How Agent Verifies |
|------|------|-------------------|
| **Rust code** | Bash (cargo) | `cargo build`, `cargo clippy`, `cargo test` |
| **WASM plugin** | Bash (cargo-component) | `cargo component build`, file exists |
| **Config** | Bash (cargo run) | Run with config, assert output |
| **End-to-end** | Bash (echo pipe) | `echo "x" \| cargo run -- --config y` |

---

## Execution Strategy

### Parallel Execution Waves

```
Wave 1 (Start Immediately):
├── Task 1: Fix Cargo.toml (edition + dependencies)
└── Task 2: Create minimal WIT contracts

Wave 2 (After Wave 1):
├── Task 3: Build Wasmtime component loader
├── Task 4: Implement SPSC queue wrapper
└── Task 7: Create TOML config parser

Wave 3 (After Wave 2):
├── Task 5: Build pass-through plugin
└── Task 6: Create pipeline executor

Wave 4 (After Wave 3):
├── Task 8: Wire end-to-end integration
└── Task 9: Add basic metrics

Wave 5 (Final):
└── Task 10: Write acceptance tests
```

### Dependency Matrix

| Task | Depends On | Blocks | Can Parallelize With |
|------|------------|--------|---------------------|
| 1 | None | 3, 4, 5, 7 | 2 |
| 2 | None | 3, 5 | 1 |
| 3 | 1, 2 | 5, 6 | 4, 7 |
| 4 | 1 | 6 | 3, 7 |
| 5 | 2, 3 | 8 | 6 |
| 6 | 3, 4 | 8 | 5 |
| 7 | 1 | 8 | 3, 4 |
| 8 | 5, 6, 7 | 9, 10 | None |
| 9 | 8 | 10 | None |
| 10 | 8, 9 | None | None |

### Agent Dispatch Summary

| Wave | Tasks | Recommended Dispatch |
|------|-------|---------------------|
| 1 | 1, 2 | Parallel: cargo-expert (1), wasm-specialist (2) |
| 2 | 3, 4, 7 | Parallel after Wave 1 completes |
| 3 | 5, 6 | Parallel after Wave 2 completes |
| 4 | 8, 9 | Sequential: 8 then 9 |
| 5 | 10 | After 9 completes |

---

## TODOs

### Task 1: Fix Cargo.toml - Edition and Dependencies

- [x] 1. Fix Cargo.toml - Edition and Dependencies

  **What to do**:
  - Change `edition = "2024"` to `edition = "2021"` (Rust 2024 doesn't exist)
  - Add missing dependencies:
    - `crossbeam-channel = "0.5"` - SPSC queue (per ADR-0002)
    - `serde = { version = "1", features = ["derive"] }` - Serialization
    - `toml = "0.8"` - Config parsing
    - `tracing = "0.1"` - Structured logging
    - `tracing-subscriber = { version = "0.3", features = ["env-filter"] }` - Log output
  - Add build dependencies section for plugin development:
    - `[workspace]` with `members = ["plugins/*"]`
  - Verify build compiles

  **Must NOT do**:
  - Do NOT add prometheus crate yet (metrics to logs first)
  - Do NOT add MQTT dependencies to main crate
  - Do NOT change wasmtime version (41.0.3 is correct)

  **Recommended Agent Profile**:
  - **Category**: `quick`
    - Reason: Single file change, well-defined modifications
  - **Skills**: [`cargo-expert`]
    - `cargo-expert`: Cargo.toml expertise, dependency management
  - **Skills Evaluated but Omitted**:
    - `wasm-specialist`: Not needed for Cargo.toml changes

  **Parallelization**:
  - **Can Run In Parallel**: YES
  - **Parallel Group**: Wave 1 (with Task 2)
  - **Blocks**: Tasks 3, 4, 5, 7 (all need dependencies)
  - **Blocked By**: None (can start immediately)

  **References**:
  
  **Pattern References**:
  - `Cargo.toml:1-12` - Current file to modify (has edition bug)
  
  **Documentation References**:
  - `docs/SPEC.md:212-223` - Technology stack with exact crate names
  - `docs/adr/0002-spsc-bounded-queues.md` - Justification for crossbeam-channel
  
  **External References**:
  - Rust editions: https://doc.rust-lang.org/edition-guide/

  **Acceptance Criteria**:

  **Agent-Executed QA Scenarios:**

  ```
  Scenario: Cargo.toml is valid and builds
    Tool: Bash (cargo)
    Preconditions: None
    Steps:
      1. cargo check 2>&1
      2. Assert: exit code 0
      3. Assert: no "edition" errors in output
    Expected Result: Project compiles without errors
    Evidence: Command output captured

  Scenario: All required dependencies present
    Tool: Bash (grep)
    Preconditions: Cargo.toml exists
    Steps:
      1. grep -q 'edition = "2021"' Cargo.toml && echo "edition OK"
      2. grep -q 'crossbeam-channel' Cargo.toml && echo "crossbeam OK"
      3. grep -q 'serde.*features.*derive' Cargo.toml && echo "serde OK"
      4. grep -q 'toml = ' Cargo.toml && echo "toml OK"
      5. grep -q 'tracing = ' Cargo.toml && echo "tracing OK"
    Expected Result: All greps succeed (exit 0)
    Evidence: Each grep output line
  ```

  **Commit**: YES
  - Message: `fix(cargo): correct edition to 2021 and add missing dependencies`
  - Files: `Cargo.toml`
  - Pre-commit: `cargo check`

---

### Task 2: Create Minimal WIT Contracts

- [x] 2. Create Minimal WIT Contracts

  **What to do**:
  - Create `wit/` directory structure
  - Create `wit/types.wit` with minimal types:
    - `message-id`, `timestamp`, `metadata` types
    - `payload` variant with ONLY `raw(list<u8>)` for MVP
    - `envelope` record (id, timestamp, source, metadata, payload)
    - `process-result` variant (emit, filter, error)
    - `process-error` record (code, message, retriable)
  - Create `wit/node.wit` with minimal lifecycle:
    - `node-config` record
    - `lifecycle` interface with ONLY `init()` (defer validate/close)
  - Create `wit/transform.wit`:
    - `transform` interface with `process(envelope) -> process-result`
    - `transform-node` world
  - Create `wit/world.wit` that composes all interfaces

  **Must NOT do**:
  - Do NOT include `validate()` or `close()` in lifecycle (defer)
  - Do NOT include Source or Sink WIT (host handles stdin/stdout directly)
  - Do NOT include Router or Joiner WIT
  - Do NOT include payload variants beyond `raw(list<u8>)`
  - Do NOT include tensor, json-value, sensor-reading types

  **Recommended Agent Profile**:
  - **Category**: `unspecified-high`
    - Reason: WIT contract design requires precision and understanding of Component Model
  - **Skills**: [`wasm-specialist`]
    - `wasm-specialist`: Expert in WIT syntax, wasmtime bindgen, Component Model
  - **Skills Evaluated but Omitted**:
    - `cargo-expert`: Not needed for WIT files

  **Parallelization**:
  - **Can Run In Parallel**: YES
  - **Parallel Group**: Wave 1 (with Task 1)
  - **Blocks**: Tasks 3, 5 (need WIT for bindgen)
  - **Blocked By**: None (can start immediately)

  **References**:
  
  **Pattern References**:
  - `docs/SPEC.md:274-435` - Full WIT types specification (use as source, simplify for MVP)
  - `docs/SPEC.md:443-491` - Node lifecycle interface
  - `docs/SPEC.md:534-568` - Transform interface
  
  **Documentation References**:
  - `docs/SPEC.md:260-272` - Package structure (pipeline:types, pipeline:node, pipeline:transform)
  
  **External References**:
  - WIT syntax: https://component-model.bytecodealliance.org/design/wit.html
  - wasmtime bindgen: https://docs.wasmtime.dev/api/wasmtime/component/macro.bindgen.html

  **Acceptance Criteria**:

  **Agent-Executed QA Scenarios:**

  ```
  Scenario: WIT files exist with correct structure
    Tool: Bash (ls, wasm-tools)
    Preconditions: None
    Steps:
      1. ls wit/*.wit
      2. Assert: types.wit, node.wit, transform.wit, world.wit exist
      3. wasm-tools component wit wit/ --json 2>&1 || echo "wasm-tools parse"
      4. Assert: No syntax errors (or install wasm-tools if missing)
    Expected Result: All WIT files present and parseable
    Evidence: File listing output

  Scenario: WIT contains required types
    Tool: Bash (grep)
    Preconditions: wit/types.wit exists
    Steps:
      1. grep -q 'record envelope' wit/types.wit
      2. grep -q 'variant process-result' wit/types.wit
      3. grep -q 'variant payload' wit/types.wit
      4. grep -q 'raw(list<u8>)' wit/types.wit
    Expected Result: All key types present
    Evidence: Grep exits 0 for each

  Scenario: WIT does NOT contain deferred types
    Tool: Bash (grep)
    Preconditions: wit/ exists
    Steps:
      1. ! grep -rq 'tensor' wit/ && echo "No tensor - OK"
      2. ! grep -rq 'json-value' wit/ && echo "No json-value - OK"
      3. ! grep -rq 'validate:' wit/ && echo "No validate - OK"
      4. ! grep -rq 'close:' wit/ && echo "No close - OK"
    Expected Result: Deferred features absent
    Evidence: Grep returns non-zero (not found)
  ```

  **Commit**: YES
  - Message: `feat(wit): add minimal WIT contracts for MVP transform pipeline`
  - Files: `wit/types.wit`, `wit/node.wit`, `wit/transform.wit`, `wit/world.wit`
  - Pre-commit: `ls wit/*.wit`

---

### Task 3: Build Wasmtime Component Loader

- [ ] 3. Build Wasmtime Component Loader

  **What to do**:
  - Create `src/engine/mod.rs` module structure
  - Create `src/engine/loader.rs`:
    - `WaferEngine` struct wrapping wasmtime `Engine` with config
    - Enable fuel metering (default 1_000_000 units)
    - Enable async support
    - `load_component(path: &Path) -> Result<Component>` function
  - Create `src/engine/host.rs`:
    - `WaferState` struct implementing `WasiView` trait
    - Contains `WasiCtx` + `ResourceTable`
    - `Store<WaferState>` creation helper
  - Create `src/engine/instance.rs`:
    - Use `wasmtime::component::bindgen!` macro with WIT files
    - `TransformInstance` wrapper for instantiated transform component
    - `call_init()` and `call_process()` methods
  - Update `src/main.rs` to export engine module

  **Must NOT do**:
  - Do NOT implement component caching/pooling (defer optimization)
  - Do NOT implement epoch interrupts (fuel only for MVP)
  - Do NOT add Source or Sink instance types (host handles I/O)

  **Recommended Agent Profile**:
  - **Category**: `unspecified-high`
    - Reason: Wasmtime Component Model + WASI P2 integration is complex
  - **Skills**: [`wasm-specialist`]
    - `wasm-specialist`: Wasmtime API expertise, async components, WasiView
  - **Skills Evaluated but Omitted**:
    - `cargo-expert`: Basic Rust, not specialized Cargo knowledge needed

  **Parallelization**:
  - **Can Run In Parallel**: YES
  - **Parallel Group**: Wave 2 (with Tasks 4, 7)
  - **Blocks**: Tasks 5, 6 (need component loader)
  - **Blocked By**: Tasks 1, 2 (needs deps + WIT)

  **References**:
  
  **Pattern References**:
  - None in codebase (greenfield)
  
  **API/Type References**:
  - `wit/world.wit` - World definition for bindgen macro (after Task 2)
  
  **Documentation References**:
  - `docs/SPEC.md:186-195` - Wasmtime Engine Pool architecture
  - `docs/adr/0001-wasmtime-runtime.md` - Runtime selection rationale
  
  **External References**:
  - wasmtime component bindgen: https://docs.wasmtime.dev/api/wasmtime/component/macro.bindgen.html
  - wasmtime-wasi WasiView: https://docs.wasmtime.dev/api/wasmtime_wasi/trait.WasiView.html
  - wasmtime fuel: https://docs.wasmtime.dev/api/wasmtime/struct.Config.html#method.consume_fuel
  - Context7 wasmtime docs for async component instantiation pattern

  **Acceptance Criteria**:

  **Agent-Executed QA Scenarios:**

  ```
  Scenario: Engine module compiles
    Tool: Bash (cargo)
    Preconditions: Tasks 1, 2 complete
    Steps:
      1. cargo check 2>&1
      2. Assert: exit code 0
      3. Assert: output contains no errors related to engine module
    Expected Result: Engine code compiles
    Evidence: cargo check output

  Scenario: Engine creates with fuel metering
    Tool: Bash (cargo test)
    Preconditions: Engine module exists
    Steps:
      1. cargo test engine::tests::test_engine_creates --no-run 2>&1
      2. Assert: test binary compiles
    Expected Result: Engine test compiles (actual test in Task 10)
    Evidence: Test compilation output

  Scenario: Bindgen macro generates types
    Tool: Bash (cargo)
    Preconditions: WIT files exist, engine module uses bindgen!
    Steps:
      1. cargo build 2>&1
      2. Assert: no "unresolved import" errors for generated types
      3. Assert: no WIT parsing errors
    Expected Result: bindgen! macro succeeds
    Evidence: Build output shows no bindgen errors
  ```

  **Commit**: YES
  - Message: `feat(engine): add wasmtime component loader with fuel metering`
  - Files: `src/engine/mod.rs`, `src/engine/loader.rs`, `src/engine/host.rs`, `src/engine/instance.rs`, `src/main.rs`
  - Pre-commit: `cargo check`

---

### Task 4: Implement SPSC Queue Wrapper

- [ ] 4. Implement SPSC Queue Wrapper

  **What to do**:
  - Create `src/queue/mod.rs` module structure
  - Create `src/queue/bounded.rs`:
    - `BoundedQueue<T>` wrapper around `crossbeam_channel::bounded`
    - Configurable capacity (default: 1024)
    - `send(&self, item: T) -> Result<()>` - blocks when full (backpressure)
    - `recv(&self) -> Result<T>` - blocks when empty
    - `try_send(&self, item: T) -> Result<(), TrySendError>` - non-blocking
    - `try_recv(&self) -> Result<T, TryRecvError>` - non-blocking
    - `len(&self) -> usize` - current queue depth (for metrics)
    - `capacity(&self) -> usize` - max capacity
  - Create `src/queue/envelope.rs`:
    - `RuntimeEnvelope` struct matching WIT envelope but as Rust type
    - Conversion functions to/from WIT-generated envelope type

  **Must NOT do**:
  - Do NOT implement custom ring buffer (crossbeam is sufficient)
  - Do NOT implement drop policy (blocking/slow only for MVP)
  - Do NOT implement MPSC variant (SPSC only)

  **Recommended Agent Profile**:
  - **Category**: `quick`
    - Reason: Wrapper around existing crate, straightforward implementation
  - **Skills**: []
    - No specialized skills needed - standard Rust
  - **Skills Evaluated but Omitted**:
    - `wasm-specialist`: Queue is host-side only, no WASM involved

  **Parallelization**:
  - **Can Run In Parallel**: YES
  - **Parallel Group**: Wave 2 (with Tasks 3, 7)
  - **Blocks**: Task 6 (pipeline executor needs queue)
  - **Blocked By**: Task 1 (needs crossbeam-channel dependency)

  **References**:
  
  **Pattern References**:
  - None in codebase (greenfield)
  
  **Documentation References**:
  - `docs/SPEC.md:717-810` - Queue and backpressure design
  - `docs/adr/0002-spsc-bounded-queues.md` - Queue implementation decision
  
  **External References**:
  - crossbeam-channel docs: https://docs.rs/crossbeam-channel/latest/crossbeam_channel/

  **Acceptance Criteria**:

  **Agent-Executed QA Scenarios:**

  ```
  Scenario: Queue module compiles
    Tool: Bash (cargo)
    Preconditions: Task 1 complete (crossbeam-channel added)
    Steps:
      1. cargo check 2>&1
      2. Assert: exit code 0
    Expected Result: Queue code compiles
    Evidence: cargo check output

  Scenario: Queue respects capacity
    Tool: Bash (cargo test)
    Preconditions: Queue module with test
    Steps:
      1. Create inline test: queue capacity 2, send 2, try_send 3rd fails
      2. cargo test queue::tests::test_capacity_limit -- --nocapture
      3. Assert: test passes
    Expected Result: Queue enforces backpressure
    Evidence: Test output showing TrySendError::Full

  Scenario: Queue send/recv works
    Tool: Bash (cargo test)
    Preconditions: Queue module with test
    Steps:
      1. Create test: send "hello", recv, assert equals "hello"
      2. cargo test queue::tests::test_send_recv
      3. Assert: test passes
    Expected Result: Basic send/recv works
    Evidence: Test passes
  ```

  **Commit**: YES
  - Message: `feat(queue): add SPSC bounded queue wrapper with backpressure`
  - Files: `src/queue/mod.rs`, `src/queue/bounded.rs`, `src/queue/envelope.rs`
  - Pre-commit: `cargo test queue`

---

### Task 5: Build Pass-Through Transform Plugin

- [ ] 5. Build Pass-Through Transform Plugin

  **What to do**:
  - Create `plugins/pass-through/` directory
  - Create `plugins/pass-through/Cargo.toml`:
    - Package name: `pass-through-transform`
    - Add `wit-bindgen = "0.36"` dependency (compatible with wasmtime 41.x)
    - Configure for WASM component target
  - Create `plugins/pass-through/src/lib.rs`:
    - Use `wit_bindgen::generate!` macro pointing to `../../wit`
    - Implement `Guest` trait for transform-node world
    - `init()` - just return Ok
    - `process(envelope)` - return `ProcessResult::Emit(envelope)` unchanged
  - Create build script or instructions for `cargo component build`
  - Output: `plugins/pass-through/target/wasm32-wasip2/release/pass_through_transform.wasm`

  **Must NOT do**:
  - Do NOT add any transformation logic (pure pass-through)
  - Do NOT add logging inside the plugin (host logs)
  - Do NOT implement validate() or close()

  **Recommended Agent Profile**:
  - **Category**: `unspecified-high`
    - Reason: WASM component compilation requires specific toolchain knowledge
  - **Skills**: [`wasm-specialist`]
    - `wasm-specialist`: wit-bindgen, cargo-component, WASM targets
  - **Skills Evaluated but Omitted**:
    - `cargo-expert`: While Cargo-related, WASM component builds are specialized

  **Parallelization**:
  - **Can Run In Parallel**: YES
  - **Parallel Group**: Wave 3 (with Task 6)
  - **Blocks**: Task 8 (integration needs the plugin)
  - **Blocked By**: Tasks 2, 3 (needs WIT files and host bindgen to verify compatibility)

  **References**:
  
  **Pattern References**:
  - None in codebase (greenfield)
  
  **API/Type References**:
  - `wit/transform.wit` - Transform interface to implement
  - `wit/types.wit` - Envelope and ProcessResult types
  
  **Documentation References**:
  - `docs/SPEC.md:543-567` - Transform interface contract
  
  **External References**:
  - wit-bindgen: https://github.com/bytecodealliance/wit-bindgen
  - cargo-component: https://github.com/bytecodealliance/cargo-component
  - wasmtime 41 + wit-bindgen compatibility: Check wit-bindgen changelog

  **Acceptance Criteria**:

  **Agent-Executed QA Scenarios:**

  ```
  Scenario: Plugin compiles to WASM component
    Tool: Bash (cargo component)
    Preconditions: Tasks 2 complete, cargo-component installed
    Steps:
      1. cd plugins/pass-through
      2. cargo component build --release 2>&1
      3. Assert: exit code 0
      4. ls target/wasm32-wasip2/release/*.wasm
      5. Assert: .wasm file exists
    Expected Result: WASM component built
    Evidence: .wasm file path

  Scenario: Plugin is valid component (not core module)
    Tool: Bash (wasm-tools)
    Preconditions: Plugin built
    Steps:
      1. wasm-tools component wit plugins/pass-through/target/wasm32-wasip2/release/pass_through_transform.wasm 2>&1
      2. Assert: output shows "world" (component, not module)
      3. Assert: no "not a component" error
    Expected Result: Valid WASM component
    Evidence: wasm-tools output showing world exports

  Scenario: Plugin size is reasonable
    Tool: Bash (ls)
    Preconditions: Plugin built
    Steps:
      1. ls -la plugins/pass-through/target/wasm32-wasip2/release/*.wasm
      2. Assert: size < 1MB (pass-through should be tiny)
    Expected Result: Plugin is small
    Evidence: File size in bytes
  ```

  **Commit**: YES
  - Message: `feat(plugin): add pass-through transform WASM component`
  - Files: `plugins/pass-through/Cargo.toml`, `plugins/pass-through/src/lib.rs`
  - Pre-commit: `cd plugins/pass-through && cargo component build --release`

---

### Task 6: Create Pipeline Executor

- [ ] 6. Create Pipeline Executor

  **What to do**:
  - Create `src/pipeline/mod.rs` module structure
  - Create `src/pipeline/executor.rs`:
    - `PipelineExecutor` struct holding:
      - `WaferEngine` reference
      - `TransformInstance` (loaded component)
      - Input/output queues (or direct I/O for MVP)
    - `run(&mut self) -> Result<()>` method:
      - Loop: read from stdin → create envelope → call transform.process() → write to stdout
      - Handle ProcessResult::Emit (output), Filter (drop), Error (log)
      - Respect fuel limits per call
      - Log metrics periodically
  - Create `src/pipeline/builder.rs`:
    - `PipelineBuilder` that constructs executor from config
    - Load component from configured path
    - Initialize node with config
  - For MVP, stdin/stdout are hardcoded (not Source/Sink components)

  **Must NOT do**:
  - Do NOT implement Source/Sink as WASM components (host handles I/O)
  - Do NOT implement multi-node pipelines (single transform only)
  - Do NOT implement parallel execution
  - Do NOT implement graceful shutdown signal handling (Ctrl+C just exits)

  **Recommended Agent Profile**:
  - **Category**: `unspecified-high`
    - Reason: Core orchestration logic, async Rust patterns
  - **Skills**: []
    - Standard Rust - no specialized domain skills
  - **Skills Evaluated but Omitted**:
    - `wasm-specialist`: Integration uses engine abstraction, not direct wasmtime

  **Parallelization**:
  - **Can Run In Parallel**: YES
  - **Parallel Group**: Wave 3 (with Task 5)
  - **Blocks**: Task 8 (integration needs executor)
  - **Blocked By**: Tasks 3, 4 (needs engine + queue)

  **References**:
  
  **Pattern References**:
  - `src/engine/instance.rs` - TransformInstance interface (from Task 3)
  - `src/queue/bounded.rs` - Queue interface (from Task 4)
  
  **Documentation References**:
  - `docs/SPEC.md:224-232` - Execution model
  - `docs/SPEC.md:179-209` - Architecture overview
  
  **External References**:
  - tokio async patterns: https://tokio.rs/tokio/tutorial

  **Acceptance Criteria**:

  **Agent-Executed QA Scenarios:**

  ```
  Scenario: Pipeline executor compiles
    Tool: Bash (cargo)
    Preconditions: Tasks 3, 4 complete
    Steps:
      1. cargo check 2>&1
      2. Assert: exit code 0
    Expected Result: Pipeline code compiles
    Evidence: cargo check output

  Scenario: Executor can be constructed
    Tool: Bash (cargo test)
    Preconditions: Pipeline module with builder test
    Steps:
      1. Create test: PipelineBuilder::new().build() doesn't panic
      2. cargo test pipeline::tests::test_builder_creates
      3. Assert: test passes
    Expected Result: Builder works
    Evidence: Test output
  ```

  **Commit**: YES
  - Message: `feat(pipeline): add pipeline executor with stdin/stdout I/O`
  - Files: `src/pipeline/mod.rs`, `src/pipeline/executor.rs`, `src/pipeline/builder.rs`
  - Pre-commit: `cargo check`

---

### Task 7: Create TOML Config Parser

- [ ] 7. Create TOML Config Parser

  **What to do**:
  - Create `src/config/mod.rs` module structure
  - Create `src/config/schema.rs`:
    - `PipelineConfig` struct (serde Deserialize):
      - `name: String` - pipeline name
      - `transform: TransformConfig` - single transform node
    - `TransformConfig` struct:
      - `name: String` - node name
      - `plugin_path: PathBuf` - path to .wasm component
      - `fuel_limit: Option<u64>` - fuel per process() call (default 1_000_000)
      - `queue_capacity: Option<usize>` - queue size (default 1024)
      - `config: Option<toml::Value>` - node-specific config (passed to init)
  - Create `src/config/loader.rs`:
    - `load_config(path: &Path) -> Result<PipelineConfig>`
    - Validate paths exist
  - Create `examples/pass-through.toml`:
    ```toml
    name = "pass-through-demo"
    
    [transform]
    name = "passthrough"
    plugin_path = "plugins/pass-through/target/wasm32-wasip2/release/pass_through_transform.wasm"
    fuel_limit = 1000000
    queue_capacity = 1024
    ```

  **Must NOT do**:
  - Do NOT support YAML or JSON (TOML only)
  - Do NOT support multiple transforms (single node only)
  - Do NOT support Source/Sink config (host handles I/O)
  - Do NOT implement config validation beyond path existence

  **Recommended Agent Profile**:
  - **Category**: `quick`
    - Reason: Straightforward serde struct + TOML parsing
  - **Skills**: []
    - Standard Rust - no specialized skills
  - **Skills Evaluated but Omitted**:
    - All - basic Rust/serde work

  **Parallelization**:
  - **Can Run In Parallel**: YES
  - **Parallel Group**: Wave 2 (with Tasks 3, 4)
  - **Blocks**: Task 8 (integration needs config)
  - **Blocked By**: Task 1 (needs serde + toml dependencies)

  **References**:
  
  **Pattern References**:
  - None in codebase (greenfield)
  
  **Documentation References**:
  - `docs/SPEC.md:1048-1150` - Pipeline configuration format
  
  **External References**:
  - serde derive: https://serde.rs/derive.html
  - toml crate: https://docs.rs/toml/latest/toml/

  **Acceptance Criteria**:

  **Agent-Executed QA Scenarios:**

  ```
  Scenario: Config module compiles
    Tool: Bash (cargo)
    Preconditions: Task 1 complete
    Steps:
      1. cargo check 2>&1
      2. Assert: exit code 0
    Expected Result: Config code compiles
    Evidence: cargo check output

  Scenario: Example config parses
    Tool: Bash (cargo test)
    Preconditions: Config module + example file
    Steps:
      1. Create test: load_config("examples/pass-through.toml") succeeds
      2. cargo test config::tests::test_example_config_parses
      3. Assert: test passes
    Expected Result: TOML parsing works
    Evidence: Test output

  Scenario: Example config file exists
    Tool: Bash (ls, cat)
    Preconditions: Example file created
    Steps:
      1. ls examples/pass-through.toml
      2. Assert: file exists
      3. cat examples/pass-through.toml
      4. Assert: contains [transform] section
    Expected Result: Example config present
    Evidence: File contents
  ```

  **Commit**: YES
  - Message: `feat(config): add TOML pipeline configuration parser`
  - Files: `src/config/mod.rs`, `src/config/schema.rs`, `src/config/loader.rs`, `examples/pass-through.toml`
  - Pre-commit: `cargo check`

---

### Task 8: Wire End-to-End Integration

- [ ] 8. Wire End-to-End Integration

  **What to do**:
  - Update `src/main.rs`:
    - Parse CLI args: `--config <path>`
    - Initialize tracing subscriber
    - Load config from TOML
    - Create WaferEngine
    - Load transform component
    - Create PipelineExecutor
    - Run executor (stdin → transform → stdout loop)
    - Log "Pipeline started" on successful init
    - Log "Pipeline stopped" on exit
  - Add clap or simple arg parsing for `--config`
  - Wire all modules together:
    - `config::load_config()`
    - `engine::WaferEngine::new()`
    - `engine::load_component()`
    - `pipeline::PipelineExecutor::new()`
    - `executor.run()`

  **Must NOT do**:
  - Do NOT add signal handling (Ctrl+C just exits)
  - Do NOT add REST API
  - Do NOT add config file watching
  - Do NOT add graceful shutdown

  **Recommended Agent Profile**:
  - **Category**: `unspecified-low`
    - Reason: Wiring existing modules together
  - **Skills**: []
    - Standard Rust integration work
  - **Skills Evaluated but Omitted**:
    - All - wiring uses existing abstractions

  **Parallelization**:
  - **Can Run In Parallel**: NO
  - **Parallel Group**: Sequential (after Wave 3)
  - **Blocks**: Tasks 9, 10
  - **Blocked By**: Tasks 5, 6, 7 (needs plugin, executor, config)

  **References**:
  
  **Pattern References**:
  - `src/engine/mod.rs` - Engine interface (from Task 3)
  - `src/pipeline/executor.rs` - Executor interface (from Task 6)
  - `src/config/loader.rs` - Config loader (from Task 7)
  
  **External References**:
  - clap (if used): https://docs.rs/clap/latest/clap/

  **Acceptance Criteria**:

  **Agent-Executed QA Scenarios:**

  ```
  Scenario: End-to-end pipeline works
    Tool: Bash (echo pipe)
    Preconditions: All previous tasks complete, plugin built
    Steps:
      1. echo "hello world" | cargo run -- --config examples/pass-through.toml 2>&1
      2. Assert: stdout contains "hello world"
      3. Assert: stderr contains "Pipeline started"
    Expected Result: Message flows through pipeline
    Evidence: Command output

  Scenario: Invalid config path fails gracefully
    Tool: Bash (cargo run)
    Preconditions: Main wired
    Steps:
      1. cargo run -- --config nonexistent.toml 2>&1
      2. Assert: exit code non-zero
      3. Assert: error message mentions config file
    Expected Result: Graceful error on bad config
    Evidence: Error message

  Scenario: Invalid plugin path fails gracefully
    Tool: Bash (cargo run)
    Preconditions: Main wired, config with bad plugin path
    Steps:
      1. Create temp config with plugin_path = "nonexistent.wasm"
      2. cargo run -- --config temp.toml 2>&1
      3. Assert: exit code non-zero
      4. Assert: error message mentions plugin/component
    Expected Result: Graceful error on bad plugin
    Evidence: Error message
  ```

  **Commit**: YES
  - Message: `feat: wire end-to-end MVP pipeline integration`
  - Files: `src/main.rs`, `Cargo.toml` (if adding clap)
  - Pre-commit: `cargo build`

---

### Task 9: Add Basic Metrics

- [ ] 9. Add Basic Metrics

  **What to do**:
  - Create `src/metrics/mod.rs` module structure
  - Create `src/metrics/counters.rs`:
    - `PipelineMetrics` struct with:
      - `messages_total: AtomicU64` - total messages processed
      - `process_time_ns: AtomicU64` - cumulative processing time
      - `queue_depth: AtomicUsize` - current queue depth
    - Methods: `increment_messages()`, `record_process_time(ns)`, `set_queue_depth(n)`
    - `report(&self) -> MetricsReport` - snapshot for logging
  - Update `PipelineExecutor` to:
    - Track metrics on each process() call
    - Log metrics every N messages (e.g., every 100) or on shutdown
  - Output format: structured tracing log
    ```
    INFO metrics: messages_total=100 avg_process_time_ns=50000 queue_depth=5
    ```

  **Must NOT do**:
  - Do NOT add Prometheus endpoint (log output only)
  - Do NOT add more than 3 metrics
  - Do NOT add histograms or percentiles (just counters/gauges)
  - Do NOT add per-node metrics (single transform only)

  **Recommended Agent Profile**:
  - **Category**: `quick`
    - Reason: Simple atomic counters and logging
  - **Skills**: []
    - Standard Rust - no specialized skills
  - **Skills Evaluated but Omitted**:
    - All - basic metrics implementation

  **Parallelization**:
  - **Can Run In Parallel**: NO
  - **Parallel Group**: Sequential (after Task 8)
  - **Blocks**: Task 10 (tests verify metrics)
  - **Blocked By**: Task 8 (needs integrated pipeline)

  **References**:
  
  **Pattern References**:
  - `src/pipeline/executor.rs` - Where to integrate metrics (from Task 6)
  
  **Documentation References**:
  - `docs/SPEC.md:1177-1240` - Observability and metrics
  
  **External References**:
  - tracing structured fields: https://docs.rs/tracing/latest/tracing/

  **Acceptance Criteria**:

  **Agent-Executed QA Scenarios:**

  ```
  Scenario: Metrics appear in logs
    Tool: Bash (cargo run + grep)
    Preconditions: Task 8 complete
    Steps:
      1. echo -e "a\nb\nc" | cargo run -- --config examples/pass-through.toml 2>&1 | grep -i "messages_total"
      2. Assert: grep finds metrics line
      3. Assert: messages_total value > 0
    Expected Result: Metrics logged
    Evidence: Log line with metrics

  Scenario: Only 3 metrics present
    Tool: Bash (grep)
    Preconditions: Metrics module exists
    Steps:
      1. grep -r "messages_total\|process_time\|queue_depth" src/metrics/
      2. Assert: all 3 found
      3. ! grep -rE "latency_p99|throughput|error_count" src/metrics/ && echo "No extra metrics"
    Expected Result: Exactly 3 metrics, no more
    Evidence: Grep results
  ```

  **Commit**: YES
  - Message: `feat(metrics): add basic pipeline metrics (messages, time, queue depth)`
  - Files: `src/metrics/mod.rs`, `src/metrics/counters.rs`, `src/pipeline/executor.rs`
  - Pre-commit: `cargo check`

---

### Task 10: Write Acceptance Tests

- [ ] 10. Write Acceptance Tests

  **What to do**:
  - Create `tests/integration.rs`:
    - `test_end_to_end_passthrough()`: echo "test" | run | assert output = "test"
    - `test_fuel_exhaustion()`: load infinite-loop plugin, assert error not hang
    - `test_queue_backpressure()`: fill queue, assert sender blocks
    - `test_invalid_component_rejected()`: load non-component .wasm, assert error
    - `test_config_parse_error()`: invalid TOML, assert error message
  - Create `tests/fixtures/` directory:
    - `tests/fixtures/invalid.wasm` - invalid bytes for rejection test
    - `tests/fixtures/bad-config.toml` - malformed TOML
  - Ensure all tests run in CI with `cargo test`

  **Must NOT do**:
  - Do NOT write unit tests for every function (focus on integration)
  - Do NOT require external services (MQTT, databases)
  - Do NOT require manual verification

  **Recommended Agent Profile**:
  - **Category**: `quick`
    - Reason: Test writing with clear requirements
  - **Skills**: []
    - Standard Rust testing
  - **Skills Evaluated but Omitted**:
    - All - basic test implementation

  **Parallelization**:
  - **Can Run In Parallel**: NO
  - **Parallel Group**: Final (after Task 9)
  - **Blocks**: None (final task)
  - **Blocked By**: Tasks 8, 9 (needs complete pipeline)

  **References**:
  
  **Pattern References**:
  - `src/main.rs` - Entry point to test (from Task 8)
  - `examples/pass-through.toml` - Config for tests
  
  **External References**:
  - Rust integration tests: https://doc.rust-lang.org/book/ch11-03-test-organization.html

  **Acceptance Criteria**:

  **Agent-Executed QA Scenarios:**

  ```
  Scenario: All integration tests pass
    Tool: Bash (cargo test)
    Preconditions: All previous tasks complete
    Steps:
      1. cargo test --test integration 2>&1
      2. Assert: exit code 0
      3. Assert: output shows "test result: ok"
    Expected Result: All tests pass
    Evidence: Test output

  Scenario: Test coverage includes key scenarios
    Tool: Bash (grep)
    Preconditions: tests/integration.rs exists
    Steps:
      1. grep -c "fn test_" tests/integration.rs
      2. Assert: count >= 5
    Expected Result: At least 5 integration tests
    Evidence: Test count
  ```

  **Commit**: YES
  - Message: `test: add integration tests for MVP pipeline`
  - Files: `tests/integration.rs`, `tests/fixtures/*`
  - Pre-commit: `cargo test`

---

## Commit Strategy

| After Task | Message | Files | Verification |
|------------|---------|-------|--------------|
| 1 | `fix(cargo): correct edition to 2021 and add missing dependencies` | Cargo.toml | cargo check |
| 2 | `feat(wit): add minimal WIT contracts for MVP transform pipeline` | wit/*.wit | ls wit/ |
| 3 | `feat(engine): add wasmtime component loader with fuel metering` | src/engine/* | cargo check |
| 4 | `feat(queue): add SPSC bounded queue wrapper with backpressure` | src/queue/* | cargo test queue |
| 5 | `feat(plugin): add pass-through transform WASM component` | plugins/pass-through/* | cargo component build |
| 6 | `feat(pipeline): add pipeline executor with stdin/stdout I/O` | src/pipeline/* | cargo check |
| 7 | `feat(config): add TOML pipeline configuration parser` | src/config/*, examples/* | cargo check |
| 8 | `feat: wire end-to-end MVP pipeline integration` | src/main.rs | cargo build |
| 9 | `feat(metrics): add basic pipeline metrics` | src/metrics/* | cargo check |
| 10 | `test: add integration tests for MVP pipeline` | tests/* | cargo test |

---

## Success Criteria

### Verification Commands
```bash
# Build succeeds
cargo build --release  # Expected: exit 0

# Clippy clean
cargo clippy -- -D warnings  # Expected: exit 0

# Tests pass
cargo test  # Expected: all tests pass

# End-to-end works
echo "hello" | cargo run -- --config examples/pass-through.toml
# Expected: "hello" on stdout, "Pipeline started" in logs

# Performance target
# (Manual benchmark, but documented command)
# Expected: p50 < 10ms at 100 msg/s
```

### Final Checklist
- [ ] All "Must Have" present:
  - [ ] WIT contracts in wit/
  - [ ] Wasmtime Engine with fuel metering
  - [ ] SPSC bounded queue
  - [ ] TOML config parser
  - [ ] Pass-through plugin
  - [ ] End-to-end integration
  - [ ] 3 basic metrics
- [ ] All "Must NOT Have" absent:
  - [ ] No Router/Joiner
  - [ ] No MQTT
  - [ ] No hot-swap
  - [ ] No REST API
  - [ ] No extra payload types
  - [ ] No more than 3 metrics
- [ ] All tests pass
- [ ] cargo clippy clean
