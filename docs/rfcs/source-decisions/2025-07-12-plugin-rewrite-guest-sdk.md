# Plugin Rewrite & Guest SDK — Session 6 Decisions

**Date:** 2025-07-12  
**Status:** Decided (pending implementation)  
**Scope:** Phase 0 structural refactor — plugin architecture, guest SDK, workspace, build, testing  
**Depends on:** Sessions 1–5 (WIT contracts, host runtime, node types, config schema, orchestrator)  
**Feeds into:** Implementation task graph, evaluation plugin builds, thesis narrative  

---

## Context

With the runtime architecture fully designed (Sessions 1–5), this session designs the plugin-side experience: how plugins are written, built, tested, and organized. The primary goals are:

1. Rewrite all plugins to match new WIT contracts (3 worlds: transform/filter/router)
2. Design a guest SDK that makes correct plugin development ergonomic
3. Structure the workspace for easy building, testing, and polyglot support
4. Build complex plugins that **justify the architecture** (not just trivial threshold checks)
5. Establish a testing pyramid from unit tests through full pipeline E2E

**Key thesis narrative:** Simple telemetry filtering doesn't need WAFER — eKuiper's SQL handles it. But modern IoT gateways increasingly run multi-vendor decoders, statistical anomaly detectors, signal processing algorithms, and ML models. These are complex, frequently-updated, potentially untrusted workloads that justify per-stage isolation and zero-downtime hot-swap.

**Binary size argument:** Wasm components are 5KB–300KB vs 50–200MB for containers. Per-stage isolation at 10,000× less overhead than Docker on constrained edge hardware.

---

## Decision 1: Plugin Inventory

**Decision:** 12 Rust plugins + 2 polyglot demos + 6 attack plugins = 20 total.

### Evaluation Plugins (Rust)

| # | Plugin | World | Evaluation Use | Deps | ~Size |
|---|--------|-------|---------------|------|-------|
| 1 | `wafer-pass-through` | transform | Pipeline C (overhead baseline) | none | ~5KB |
| 2 | `wafer-json-parse` | transform | Pipeline A (telemetry) | manual parsing | ~8KB |
| 3 | `wafer-uppercase` | transform | Unit test / demo | none | ~5KB |
| 4 | `wafer-threshold-filter` | filter | Pipeline A, eKuiper comparison | manual parsing | ~8KB |
| 5 | `wafer-content-router` | router | Pipeline A | manual parsing | ~8KB |
| 6 | `wafer-tensor-prep` | transform | Pipeline B (inference) | none | ~6KB |
| 7 | `wafer-result-format` | transform | Pipeline B | none | ~8KB |
| 8 | `wafer-mnist-inference` | inference | Pipeline B, E-Swap-* | wasi:nn | ~20KB |

### Complex Plugins — Architecture Justification (Rust)

| # | Plugin | World | Justifies | Deps | ~Size |
|---|--------|-------|-----------|------|-------|
| 9 | `wafer-cayenne-decoder` | transform | Multi-vendor sandbox | none | ~10KB |
| 10 | `wafer-anomaly-detector` | transform | Stateful + hot-swap recalibration | serde_json | ~160KB |
| 11 | `wafer-vibration-features` | transform | Complex DSP + vendor code | rustfft, serde_json | ~300KB |
| 12 | `wafer-quality-rules` | filter | Frequently-changing logic | serde_json | ~150KB |

### Polyglot Demos

| # | Plugin | Language | World | Purpose |
|---|--------|----------|-------|---------|
| 13 | `wafer-uppercase-go` | Go (TinyGo) | transform | Polyglot proof |
| 14 | `wafer-threshold-filter-py` | Python | filter | Polyglot proof |

### Attack Scenario Plugins (S1–S6)

| ID | Plugin | Attack | Expected |
|----|--------|--------|----------|
| S1 | `wafer-attack-buffer-overflow` | Write past linear memory bounds | Trap (OOB) |
| S2 | `wafer-attack-cross-read` | Attempt host memory access | Impossible |
| S3 | `wafer-attack-infinite-loop` | `loop {}` | Epoch interrupt |
| S4 | `wafer-attack-memory-exhaust` | Allocate until limit | Trap |
| S5 | `wafer-attack-fs-access` | Unauthorized WASI FS call | Trap |
| S6 | `wafer-attack-panic` | `panic!("boom")` | Node traps, pipeline continues |

### Deleted

| Plugin | Reason |
|--------|--------|
| `merge-joiner` | Joiner world removed (Session 3 A1) |
| `filter` (as transform) | Replaced by `wafer-threshold-filter` (filter-node world) |

---

## Decision 2: State Storage Pattern — `RefCell` via `thread_local!`

**Decision:** Plugins use `thread_local!` + `RefCell<Option<T>>` for mutable state. No `unsafe` in user plugin code.

**Rationale:**
- `OnceLock` is read-only after init — doesn't work for EWMA accumulators
- `static mut` requires unsafe at every access (error-prone, Clippy warns)
- `RefCell<Option<T>>` gives safe interior mutability in single-threaded Wasm
- `thread_local!` is the idiomatic Rust way; compiles to a simple global in Wasm (single-threaded)
- Component Model spec (issue #364) documents this as the intended pattern

**SDK provides macros:**
```rust
define_state!(MyState);                    // declares the thread_local RefCell
set_state!(value);                         // sets state in init()
with_state!(name => { /* use name */ });   // borrows state for use in process/evaluate/route
```

**Hot-swap interaction:** State is lost on hot-swap (new Store = fresh linear memory). This is BY DESIGN:
- EWMA recalibration: state SHOULD reset (new baseline for new conditions)
- Code fix: state loss is acceptable (better correct code than preserved buggy state)
- State persistence across swaps is explicitly future work (`pipeline:host/kv-store`)

---

## Decision 3: Guest SDK — `wafer-plugin` Crate

**Decision:** Thin helper crate providing output construction, error helpers, state macros, payload access, and config parsing. Internal path dependency (not designed for crates.io publication). No proc macros.

**API surface — `macro_rules!` macros + utility functions:**

> **IMPORTANT DESIGN NOTE:** Most SDK helpers are `macro_rules!` macros, NOT regular functions.
> Reason: WIT types (`Message`, `OutputMessage`, `ProcessError`) are generated locally in each
> plugin by `wit_bindgen::generate!`. A separate crate cannot reference those types as they don't
> exist until each plugin generates them. Macros expand at the call site where types are in scope.
> Only type-agnostic utilities (`parse_config`) are regular functions.

```rust
// === Output Construction (macros — expand where OutputMessage type is in scope) ===

/// Preserve input identity fields, replace payload.
#[macro_export]
macro_rules! output_from {
    ($input:expr, $payload:expr) => {{
        let __input = &$input;
        OutputMessage {
            id: __input.id.clone(),
            timestamp: __input.timestamp,
            source: __input.source.clone(),
            content_type: __input.content_type.clone(),
            metadata: __input.metadata.clone(),
            payload: $payload,
        }
    }};
}

/// Preserve input identity fields, replace payload AND content-type.
#[macro_export]
macro_rules! output_with_type {
    ($input:expr, $payload:expr, $content_type:expr) => {{
        let __input = &$input;
        OutputMessage {
            id: __input.id.clone(),
            timestamp: __input.timestamp,
            source: __input.source.clone(),
            content_type: $content_type.to_string(),
            metadata: __input.metadata.clone(),
            payload: $payload,
        }
    }};
}

// === Payload Access (macros — call methods on WIT resource type) ===

/// Read full payload bytes from borrow<buffer>. One copy: host → linear memory.
#[macro_export]
macro_rules! payload_bytes {
    ($input:expr) => { $input.payload.read_all() };
}

/// Read payload as UTF-8 string. Returns ProcessError::BadInput on invalid UTF-8.
#[macro_export]
macro_rules! payload_as_str {
    ($input:expr) => {{
        let bytes = $input.payload.read_all();
        String::from_utf8(bytes).map_err(|_| bad_input!("payload is not valid UTF-8"))
    }};
}

// === Error Construction (macros — ProcessError variant is generated locally) ===
//
// Decision guide: "WHY did processing fail?"
// - Input is wrong (malformed, missing fields) → bad_input! [DLQ, no retry]
// - External resource down (DB, API, network) → dependency_failed! [retry + backoff]
// - My code hit unexpected condition → processing_failed! [retry N times, then DLQ]
// - Can't recover at all (corrupted state) → unrecoverable! [teardown + re-instantiate]
// Note: timed_out is generated by HOST (epoch interrupt), never by plugin code.

#[macro_export]
macro_rules! bad_input {
    ($reason:expr) => { ProcessError::BadInput($reason.to_string()) };
}
#[macro_export]
macro_rules! dependency_failed {
    ($reason:expr) => { ProcessError::DependencyFailed($reason.to_string()) };
}
#[macro_export]
macro_rules! processing_failed {
    ($reason:expr) => { ProcessError::ProcessingFailed($reason.to_string()) };
}
#[macro_export]
macro_rules! unrecoverable {
    ($reason:expr) => { ProcessError::Unrecoverable($reason.to_string()) };
}

// === Config Parsing (regular function — no WIT types involved) ===

/// Parse JSON config string into a typed struct. Use in init().
/// Only available with `features = ["serde"]`.
#[cfg(feature = "serde")]
pub fn parse_config<T: serde::de::DeserializeOwned>(json: &str) -> Result<T, String> {
    serde_json::from_str(json).map_err(|e| format!("config parse error: {e}"))
}

// === State Management (macros — declare and access plugin state safely) ===

/// Declare a thread-local RefCell for plugin state. Call once at module level.
#[macro_export]
macro_rules! define_state {
    ($type:ty) => {
        thread_local! {
            static __WAFER_STATE: std::cell::RefCell<Option<$type>> =
                std::cell::RefCell::new(None);
        }
    };
}

/// Set plugin state (call in init()). Overwrites any previous state.
#[macro_export]
macro_rules! set_state {
    ($val:expr) => {
        __WAFER_STATE.with(|cell| *cell.borrow_mut() = Some($val))
    };
}

/// Borrow plugin state mutably for use in process/evaluate/route.
/// Panics if init() was not called (programming error).
#[macro_export]
macro_rules! with_state {
    ($name:ident => $body:expr) => {
        __WAFER_STATE.with(|cell| {
            let mut borrow = cell.borrow_mut();
            let $name = borrow.as_mut()
                .expect("plugin not initialized: init() must be called first");
            $body
        })
    };
}

// === Logging (macros — call pipeline:host/logging import) ===
// These expand to calls on the generated logging interface.
#[macro_export]
macro_rules! log_info {
    ($msg:expr) => { pipeline::host::logging::log(LogLevel::Info, $msg) };
}
#[macro_export]
macro_rules! log_warn {
    ($msg:expr) => { pipeline::host::logging::log(LogLevel::Warn, $msg) };
}
#[macro_export]
macro_rules! log_error {
    ($msg:expr) => { pipeline::host::logging::log(LogLevel::Error, $msg) };
}
```

**Why `macro_rules!` (not proc macros, not functions):**
- **Not functions:** WIT types are generated locally per-plugin — a separate crate can't reference them
- **Not proc macros:** Zero compile-time cost, debuggable via `cargo expand`, no separate crate needed
- **macro_rules! works:** Expands at call site where all WIT types are in scope. Simple pattern substitution.
- `parse_config<T>` is the ONE regular function — it only uses `&str` and `String`, no WIT types

**Dependency strategy:**
- `wafer-plugin` itself depends only on `serde`+`serde_json` behind feature flag
- Simple plugins: `wafer-plugin` (no features) → zero transitive deps, macros only
- Complex plugins: `wafer-plugin` with `features = ["serde"]` → enables `parse_config<T>`

---

## Decision 4: Workspace Structure

**Decision:** Separate `plugins/` workspace (different build target). Polyglot plugins in language-specific subdirectories. SDK crate in host workspace.

```
wafer/
├── Cargo.toml                      # Host workspace (wafer-core, wafer-runtime, etc.)
├── crates/
│   ├── wafer-core/
│   ├── wafer-runtime/
│   ├── wafer-types/
│   ├── waferctl/
│   └── wafer-plugin/               # Guest SDK (internal, path dep from plugins)
├── plugins/
│   ├── Cargo.toml                  # Plugin workspace (all Rust plugins)
│   ├── .cargo/config.toml          # [build] target = "wasm32-wasip2"
│   ├── wit/                        # New 4-package WIT (source of truth)
│   │   ├── pipeline-types.wit
│   │   ├── pipeline-node.wit
│   │   ├── pipeline-routing.wit
│   │   ├── pipeline-host.wit
│   │   └── deps/wasi-nn/
│   ├── pass-through/
│   ├── json-parse/
│   ├── uppercase/
│   ├── threshold-filter/
│   ├── content-router/
│   ├── tensor-prep/
│   ├── result-format/
│   ├── mnist-inference/
│   ├── cayenne-decoder/
│   ├── anomaly-detector/
│   ├── vibration-features/
│   ├── quality-rules/
│   ├── attacks/
│   │   ├── buffer-overflow/
│   │   ├── infinite-loop/
│   │   ├── memory-exhaust/
│   │   ├── fs-access/
│   │   └── panic/
│   ├── go/
│   │   └── uppercase/              # TinyGo polyglot demo
│   │       ├── main.go
│   │       ├── go.mod
│   │       └── Makefile
│   └── python/
│       └── threshold-filter/       # componentize-py polyglot demo
│           ├── app.py
│           └── Makefile
└── wit/                            # Symlink or copy to plugins/wit/
```

**Rationale:**
- Host and plugins have different targets — can't share a workspace
- `.cargo/config.toml` means plain `cargo build` targets wasm32-wasip2
- No `cargo-component` needed (wasm32-wasip2 is tier-2 stable since Rust 1.82)
- Polyglot plugins have their own toolchains (TinyGo, componentize-py)

---

## Decision 5: Build Tooling

**Decision:** Makefile at `plugins/` root (temporary — will migrate to mise). Includes `wasm-opt` post-processing for release builds.

```makefile
# plugins/Makefile
.PHONY: all release clean

all:
	cargo build

release:
	cargo build --release
	# Post-process: 10-30% size reduction
	find target/wasm32-wasip2/release -name "*.wasm" -exec wasm-opt -Os {} -o {} \;

clean:
	cargo clean
```

**Per-plugin Cargo.toml (simple, no serde):**
```toml
[package]
name = "wafer-pass-through"
version = "0.1.0"
edition = "2024"

[lib]
crate-type = ["cdylib"]

[dependencies]
wit-bindgen = "0.53"
wafer-plugin = { path = "../../crates/wafer-plugin" }

[profile.release]
opt-level = "s"
lto = true
strip = "debuginfo"
```

**Per-plugin Cargo.toml (complex, with serde):**
```toml
[package]
name = "wafer-anomaly-detector"
version = "0.1.0"
edition = "2024"

[lib]
crate-type = ["cdylib"]

[dependencies]
wit-bindgen = "0.53"
wafer-plugin = { path = "../../crates/wafer-plugin", features = ["serde"] }
serde = { version = "1", features = ["derive"] }
serde_json = "1"

[profile.release]
opt-level = "s"
lto = true
strip = "debuginfo"
```

**WIT binding generation (in each plugin's lib.rs):**
```rust
wit_bindgen::generate!({
    path: "../wit",
    world: "transform-node",  // or "filter-node" / "router-node" / "inference-node"
});
```

---

## Decision 6: Testing Strategy — Three Levels + E2E

**Decision:** Three-level testing pyramid. Plugin test harness and E2E pipeline tests are first-class infrastructure (critical for LLM-assisted development).

### Level 1: Unit Tests (native target, in plugin crate)

```rust
// Run: cd plugins && cargo test --target x86_64-unknown-linux-gnu
#[cfg(test)]
mod tests {
    use super::*;
    
    #[test]
    fn ewma_detects_spike() {
        let mut acc = EwmaAccumulator::new();
        for _ in 0..100 { acc.update(10.0, 0.1); }
        assert!(acc.z_score(100.0) > 3.0);
    }
}
```

### Level 2: Plugin Integration Tests (host harness)

```rust
// crates/wafer-core/src/testing.rs — PluginTestHarness
pub struct PluginTestHarness { engine: Engine }

impl PluginTestHarness {
    pub fn new() -> Self;
    pub async fn transform(wasm_path: &str, config: &str) -> Result<TransformTestInstance>;
    pub async fn filter(wasm_path: &str, config: &str) -> Result<FilterTestInstance>;
    pub async fn router(wasm_path: &str, config: &str) -> Result<RouterTestInstance>;
}

impl TransformTestInstance {
    pub async fn process(&mut self, payload: &[u8]) -> Result<Vec<u8>>;
    pub async fn process_message(&mut self, msg: TestMessage) -> Result<TestOutput>;
    pub async fn process_expect_error(&mut self, payload: &[u8]) -> Result<ProcessError>;
}

impl FilterTestInstance {
    pub async fn evaluate(&mut self, payload: &[u8]) -> Result<bool>;
}

impl RouterTestInstance {
    pub async fn route(&mut self, payload: &[u8]) -> Result<Vec<String>>;
    pub fn output_ports(&self) -> &[String];
}
```

### Level 3: Pipeline E2E Tests

```rust
// crates/wafer-core/tests/pipeline_e2e.rs — TestPipeline
let outputs = TestPipeline::new()
    .source(vec![json!({...}), json!({...})])
    .transform("parse", ".../wafer_json_parse.wasm", "")
    .filter("threshold", ".../wafer_threshold_filter.wasm", config)
    .router("router", ".../wafer_content_router.wasm", config)
    .sink("alert", port: "alert")
    .sink("log", port: "log")
    .run()
    .await
    .unwrap();

assert_eq!(outputs["alert"].len(), expected_alerts);
assert_eq!(outputs["log"].len(), expected_logs);
```

**TestPipeline internals:** Uses the REAL orchestrator/builder/runner loops from Session 5, but with:
- `MemorySource`: emits from `Vec<Vec<u8>>`
- `CollectorSink`: captures to `Arc<Mutex<Vec<RuntimeEnvelope>>>`
- Same queues, same error policy, same hot-swap — only I/O is mocked

**E2E tests cover:**
- Pipeline A (telemetry): source → parse → filter → router → sinks
- Pipeline B (inference): source → tensor-prep → mnist → result-format → sink
- Pipeline C (passthrough): source → pass-through → sink (zero-loss verification)
- Hot-swap: continuous source, swap mid-stream, assert zero loss/duplication
- Attack containment: bad node traps, good nodes continue

---

## Decision 7: Content Router Logic

**Decision:** Router reads payload via `buffer.read_all()`, parses JSON, routes on configurable field. Returns `list<port-id>`.

```rust
fn route(input: Message) -> Result<Vec<PortId>, ProcessError> {
    let bytes = input.payload.read_all();
    let port = match extract_json_field(&bytes, &self.route_field) {
        Some(value) => self.rules.get(value).unwrap_or(&self.default_port),
        None => &self.default_port,
    };
    Ok(vec![port.clone()])
}
```

**Simple plugins use manual JSON extraction** (like current content-router's `extract_route_field` pattern) to stay dependency-free and under 10KB.

---

## Decision 8: Naming Convention

**Decision:** `wafer-{descriptive-name}` for all plugin crate names.

Examples: `wafer-pass-through`, `wafer-threshold-filter`, `wafer-anomaly-detector`, `wafer-cayenne-decoder`.

---

## Decision 9: Attack Plugins (S1–S6)

**Decision:** Structure and Cargo.toml created now. Implementation bodies added during evaluation session (each is 5–20 lines).

All target `transform-node` world. Each implements `process()` with pathological behavior to verify containment.

---

## Decision 10: Transform DX

**Decision:** `output_from(&input, payload)` — single function preserving input identity. No builder pattern.

```rust
fn process(input: Message) -> Result<OutputMessage, ProcessError> {
    let bytes = payload_bytes(&input);
    let result = transform_logic(bytes)?;
    Ok(output_from(&input, result))
}
```

---

## Decision 11: Filter DX

**Decision:** Explicit `impl Guest` with `evaluate() → Result<bool, ProcessError>`. SDK provides `payload_as_str()` and `payload_bytes()`.

```rust
fn evaluate(input: Message) -> Result<bool, ProcessError> {
    with_state!(cfg => {
        let value = extract_field_manual(&payload_bytes(&input), &cfg.field)?;
        Ok(value >= cfg.min && value <= cfg.max)
    })
}
```

---

## Decision 12: Error Categorization DX

**Decision:** Constructor functions with decision-guide documentation.

```rust
/// Ask: "WHY did processing fail?"
/// - Input is wrong → bad_input() [DLQ, no retry]
/// - External resource down → dependency_failed() [retry with backoff]
/// - My code hit unexpected condition → processing_failed() [retry N times]
/// - Can't recover at all → unrecoverable() [teardown + re-instantiate]
/// Note: timed_out is generated by HOST (epoch interrupt), never by plugin.
```

---

## Decision 13: wasi-nn Integration

**Decision:** Inference plugin targets `inference-node` world. wit-bindgen generates all bindings from WIT (including wasi:nn imports). Model loaded via `wasi:nn/graph::load_by_name()` for hot-swap support.

---

## Decision 14: Polyglot Plugins

**Decision:** Go (TinyGo) uppercase + Python (componentize-py) threshold-filter. Demo-only (not in timed experiments). Proves Component Model polyglot composition.

**Thesis narrative:** "Same WIT contract, different source languages, same pipeline. Go developers (eKuiper's language) and Python data scientists can write WAFER plugins without learning Rust."

---

## Decision 15: Complex Plugin Scope & Priority

**Decision:** Build all 4 complex plugins (Cayenne decoder, EWMA anomaly detector, vibration features, quality rules). EWMA first (strongest hot-swap justification story), others built incrementally.

**Justification mapping:**

| Plugin | Justifies Wasm Isolation | Justifies Hot-Swap | Complexity |
|--------|-------------------------|-------------------|-----------|
| `wafer-cayenne-decoder` | Third-party binary decoder can't crash host | New sensor type table from vendor | ~150 lines, TLV binary parsing |
| `wafer-anomaly-detector` | Statistical code with NaN/division risks | Recalibration after maintenance | ~200 lines, EWMA + z-score |
| `wafer-vibration-features` | Complex DSP from equipment vendors | New frequency bands for different machine | ~200 lines, FFT + feature extraction |
| `wafer-quality-rules` | Rule bugs shouldn't crash gateway | New product spec per shift | ~100 lines, configurable multi-field rules |

---

## Decision 16: Binary Size Strategy

**Decision:** Simple plugins avoid serde (manual JSON parsing, ~5-10KB). Complex plugins use serde (~150-300KB). All get `wasm-opt -Os` post-processing.

**Size comparison (thesis argument):**

| Isolation Method | Simple Filter | Complex Transform |
|-----------------|--------------|-------------------|
| Docker container | ~50-100 MB | ~100-200 MB |
| WAFER Wasm (no serde) | **~5 KB** | — |
| WAFER Wasm (with serde) | — | **~150-300 KB** |

10,000× smaller than containers for simple plugins. 300-600× smaller for complex ones.

---

## Decision 17: E2E Pipeline Tests

**Decision:** `TestPipeline` builder using real runtime with mocked I/O. First-class testing infrastructure for correctness verification and LLM-assisted development.

Covers: all evaluation pipelines (A/B/C), hot-swap zero-loss, attack containment, polyglot interop.

---

## Complex Plugin Sketches

### wafer-anomaly-detector (Transform, ~200 lines)

```rust
// EWMA-based anomaly detection with configurable per-field tracking
// Config: { "alpha": 0.1, "warmup_samples": 50, "z_threshold": 3.0, "fields": ["temperature", "pressure"] }
// Input: {"temperature": 85.2, "pressure": 4.1, ...}
// Output: {"temperature": 85.2, "pressure": 4.1, "anomaly_score": 3.7, "severity": "warning", "field_scores": {"temperature": 3.7, "pressure": 0.2}}
//
// Hot-swap story: Maintenance team replaced bearing on pump #3.
// Deploy fresh detector with new alpha for break-in period.
// State resets (new Store) = new baseline. By design.
```

### wafer-cayenne-decoder (Transform, ~150 lines)

```rust
// Decodes Cayenne Low Power Payload binary format to structured JSON
// Config: { "extra_types": { "200": { "name": "custom_sensor", "size": 4, "scale": 0.01 } } }
// Input: raw bytes [01 67 00FF 02 68 80 ...] (Cayenne LPP)
// Output: {"sensors": [{"channel": 1, "type": "temperature", "value": 25.5, "unit": "°C"}, ...]}
//
// Hot-swap story: Vendor ships firmware update with new sensor type (channel 200).
// Deploy updated decoder with new type table. Zero downtime.
```

### wafer-vibration-features (Transform, ~200 lines)

```rust
// FFT-based vibration feature extraction for predictive maintenance
// Config: { "sample_rate": 10000, "fft_size": 1024, "bands": [[0,100], [100,1000], [1000,5000]] }
// Input: 4096 bytes (1024 × f32 accelerometer samples, little-endian)
// Output: {"rms": 2.3, "peak": 8.1, "crest_factor": 3.5, "kurtosis": 4.1,
//          "dominant_freq": 847, "bands": [0.5, 1.8, 0.3], "health_score": 0.73}
//
// Hot-swap story: Machine speed changes from 3000 RPM to 1500 RPM.
// Deploy updated plugin with new frequency bands of interest.
// Dep: rustfft (pure Rust, has Wasm SIMD support)
```

### wafer-quality-rules (Filter, ~100 lines)

```rust
// Configurable multi-field quality inspection rules
// Config: { "rules": [
//   {"field": "diameter", "op": "range", "min": 9.9, "max": 10.1},
//   {"field": "surface_roughness", "op": "less_than", "value": 0.8}
// ], "logic": "all" }
// Input: {"diameter": 10.05, "surface_roughness": 0.6, ...}
// Output: true (pass) or false (reject)
//
// Hot-swap story: Production switches from Product A to Product B.
// Quality engineer deploys new rule set for different tolerances.
// Happens 2-5× per day on active manufacturing lines.
```

---

## Hello World: Complete Plugin Per Type

### Transform (minimal, no deps, ~5KB)

```rust
wit_bindgen::generate!({ path: "../wit", world: "transform-node" });
use wafer_plugin::{output_from, payload_bytes};

struct PassThrough;

impl exports::pipeline::node::lifecycle::Guest for PassThrough {
    fn validate(_config: NodeConfig) -> Option<String> { None }
    fn init(_config: NodeConfig) -> Result<(), ProcessError> { Ok(()) }
    fn close() {}
}

impl exports::pipeline::node::transform::Guest for PassThrough {
    fn process(input: Message) -> Result<OutputMessage, ProcessError> {
        Ok(output_from(&input, payload_bytes(&input)))
    }
}

export!(PassThrough);
```

### Filter (minimal, no deps, ~5KB)

```rust
wit_bindgen::generate!({ path: "../wit", world: "filter-node" });

struct PassAll;

impl exports::pipeline::node::lifecycle::Guest for PassAll {
    fn validate(_config: NodeConfig) -> Option<String> { None }
    fn init(_config: NodeConfig) -> Result<(), ProcessError> { Ok(()) }
    fn close() {}
}

impl exports::pipeline::node::filter::Guest for PassAll {
    fn evaluate(_input: Message) -> Result<bool, ProcessError> {
        Ok(true)
    }
}

export!(PassAll);
```

### Router (minimal, no deps, ~5KB)

```rust
wit_bindgen::generate!({ path: "../wit", world: "router-node" });

struct DefaultRouter;

impl exports::pipeline::node::lifecycle::Guest for DefaultRouter {
    fn validate(_config: NodeConfig) -> Option<String> { None }
    fn init(_config: NodeConfig) -> Result<(), ProcessError> { Ok(()) }
    fn close() {}
}

impl exports::pipeline::routing::router::Guest for DefaultRouter {
    fn output_ports() -> Vec<PortId> { vec!["default".into()] }
    fn route(_input: Message) -> Result<Vec<PortId>, ProcessError> {
        Ok(vec!["default".into()])
    }
}

export!(DefaultRouter);
```

---

## Implementation Priority Order

1. **`wafer-plugin` SDK crate** — foundation for everything else
2. **WIT files** — rewrite to 4-package structure (Session 1)
3. **`wafer-pass-through`** — simplest plugin, validates the entire build chain works
4. **`PluginTestHarness`** — Level 2 testing (verify pass-through works against WIT boundary)
5. **`wafer-threshold-filter`** — first filter-node plugin
6. **`wafer-content-router`** — first router-node plugin
7. **`TestPipeline` E2E** — Level 3 testing (wire a simple pipeline)
8. **`wafer-json-parse`** — Pipeline A complete
9. **`wafer-anomaly-detector`** — first complex plugin (strongest narrative)
10. **Pipeline A E2E test** — full telemetry pipeline verified
11. **`wafer-tensor-prep` + `wafer-result-format` + `wafer-mnist-inference`** — Pipeline B
12. **Remaining complex plugins** — Cayenne, vibration, quality-rules
13. **Polyglot plugins** — Go + Python demos
14. **Attack plugins** — S1–S6 (trivial bodies)

---

## What This Does NOT Decide (Future Work)

- `.cwasm` precompilation cache (not needed for thesis eval — pipeline starts once per experiment)
- Guest-side buffer pooling (micro-optimization, negligible vs WIT boundary cost)
- State serialization across hot-swaps (`pipeline:host/kv-store` import)
- Plugin marketplace / signing / versioning (post-thesis)
- Additional host imports (metrics, http-client, kv-store)
- `waferctl new plugin` scaffolding command

---

## Sources Consulted

| Source | What it informed |
|--------|-----------------|
| Torvyn examples (4 guest plugins) | D2 (static mut pattern), D3 (no proc macros), D4 (workspace structure) |
| Azure IoT dataflow-graphs (filter, map, custom) | D2 (OnceLock for config), D3 (proc macro trade-offs), D11 (filter DX) |
| Fluvio SmartModules (regex-filter, map) | D2 (OnceLock), D3 (smartmodule derive), D12 (eyre errors) |
| WebAssembly/component-model#364 | D2 (mutable global state is the intended pattern) |
| CM spec UseCases.md | D2 (retained mutable state across export calls) |
| component-model.bytecodealliance.org (Rust guide) | D4, D5 (cargo build --target wasm32-wasip2) |
| Rust blog "wasm32-wasip2 Tier 2" (2024) | D5 (no cargo-component needed) |
| rustfft docs (Wasm SIMD support) | D15 (vibration-features feasibility confirmed) |
| wit-bindgen docs (generate! macro) | D5, D13 (path-based WIT resolution) |
| anomstream-core (per_feature_ewma.rs) | D15 (EWMA implementation pattern, ~100 lines for core algorithm) |
| Cayenne LPP specification | D15 (binary TLV format for decoder plugin) |
| bytecodealliance/go-modules | D14 (TinyGo CM support) |
| componentize-py | D14 (Python CM support, mature tooling) |
| ThingsBoard data-converters (200+ decoders) | D1 (multi-vendor decoder ecosystem justification) |
| Yantrix MLOps case study (600 Jetson cameras) | D1 (model hot-swap at scale justification) |
| Edge computing processing patterns (IoT class) | D1 (filter/aggregate/infer/store-forward taxonomy) |
| eKuiper rules engine documentation | D1 (SQL-level operations = baseline comparison) |
| IIoT Edge Gateway Architecture 2026 | D1 (protocol translation, store-and-forward, edge compute slots) |
| microsoft/wassette#615 (persistent Store) | D2 (per-session state = WAFER's persistent Store model) |
| wasmtime Store docs | D2 (linear memory lifetime = Store lifetime) |
| Web: IoT data pipeline patterns | D1 (validate/decode/enrich/filter/route/aggregate) |
| IEEE: Vibration-based fault detection IoT | D15 (FFT + statistical features at edge) |
| SCORED 2023: Material identification via Wasm + vibration | D15 (academic precedent for Wasm + signal processing at edge) |
