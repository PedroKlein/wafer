# RFC-006: Plugin Rewrite & Guest SDK

- **Status:** Implemented
- **Original session date:** 2026-07-12
- **Amends:** —
- **Amended by:** —

## Abstract

WAFER's plugin-side developer experience needed a complete redesign to match the post-refactor WIT contracts (three processing worlds: transform-node, filter-node, router-node) and to enable both trivial telemetry plugins and complex multi-vendor workloads that justify per-stage isolation. This RFC defines the plugin inventory (12 Rust evaluation plugins + 2 polyglot demos + 6 attack-scenario plugins), a guest SDK crate (`wafer-plugin`) built on `macro_rules!` macros (not proc macros) and a `thread_local! + RefCell<Option<T>>` state pattern, a workspace structure separating host and plugin build targets, a three-level testing pyramid (unit → plugin-harness → pipeline E2E), and conventions for naming, binary size optimisation, and error categorisation. The design explicitly avoids state preservation across hot-swaps and keeps the SDK internal (not published to crates.io).

## Context

With the runtime architecture fully designed (Sessions 1–5 covering WIT contracts, host runtime, node types, config schema, and orchestrator), this session designed the plugin-side experience: how plugins are written, built, tested, and organized. The primary goals were:

1. Rewrite all plugins to match new WIT contracts (3 worlds: transform/filter/router, plus inference variant)
2. Design a guest SDK that makes correct plugin development ergonomic
3. Structure the workspace for easy building, testing, and polyglot support
4. Build complex plugins that **justify the architecture** (not just trivial threshold checks)
5. Establish a testing pyramid from unit tests through full pipeline E2E

The key thesis narrative motivating complex plugins: simple telemetry filtering doesn't need WAFER — eKuiper's SQL handles it. But modern IoT gateways increasingly run multi-vendor decoders, statistical anomaly detectors, signal processing algorithms, and ML models. These are complex, frequently-updated, potentially untrusted workloads that justify per-stage isolation and zero-downtime hot-swap.

Binary size provides an additional argument: Wasm components are 5KB–300KB versus 50–200MB for containers — 10,000× less overhead for per-stage isolation on constrained edge hardware.

## Decisions

### Decision 1: Plugin Inventory

20 total plugins: 12 Rust evaluation plugins + 2 polyglot demos + 6 attack-scenario plugins.

**Evaluation Plugins (Rust):**
- `wafer-pass-through` (transform) — overhead baseline, ~5KB
- `wafer-json-parse` (transform) — telemetry pipeline, ~8KB
- `wafer-uppercase` (transform) — unit test / demo, ~5KB
- `wafer-threshold-filter` (filter) — eKuiper comparison, ~8KB
- `wafer-content-router` (router) — telemetry routing, ~8KB
- `wafer-tensor-prep` (transform) — inference pipeline, ~6KB
- `wafer-result-format` (transform) — inference pipeline, ~8KB
- `wafer-mnist-inference` (inference) — wasi:nn, ~20KB

**Complex Plugins — Architecture Justification (Rust):**
- `wafer-cayenne-decoder` (transform) — multi-vendor sandbox justification, ~10KB
- `wafer-anomaly-detector` (transform) — stateful + hot-swap recalibration, ~160KB
- `wafer-vibration-features` (transform) — complex DSP, ~300KB
- `wafer-quality-rules` (filter) — frequently-changing logic, ~150KB

**Polyglot Demos:** Go (TinyGo) uppercase + Python (componentize-py) threshold-filter.

**Attack Scenario Plugins (S1–S6):** buffer-overflow, cross-read, infinite-loop, memory-exhaust, fs-access, panic.

**Deleted:** `merge-joiner` (Joiner world removed in RFC-003 §A1); `filter` as transform (replaced by `wafer-threshold-filter` on filter-node world).

### Decision 2: State Storage Pattern — `RefCell` via `thread_local!`

Plugins use `thread_local!` + `RefCell<Option<T>>` for mutable state. No `unsafe` in user plugin code.

Rationale: `OnceLock` is read-only after init; `static mut` requires unsafe; `RefCell<Option<T>>` gives safe interior mutability in single-threaded Wasm; the Component Model spec (issue #364) documents this as the intended pattern.

State is lost on hot-swap (new Store = fresh linear memory). This is by design — EWMA accumulators should reset after maintenance, and code-fix deployments don't need old buggy state.

### Decision 3: Guest SDK — `wafer-plugin` Crate

Thin helper crate providing output construction, error helpers, state macros, payload access, and config parsing. Internal path dependency; no proc macros.

Most SDK helpers are `macro_rules!` macros because WIT types (`Message`, `OutputMessage`, `ProcessError`) are generated locally in each plugin by `wit_bindgen::generate!` — a separate crate cannot reference those types. Macros expand at the call site where types are in scope.

Key macros:
- `output_from!` / `output_with_type!` — preserve input identity fields, replace payload
- `payload_bytes!` / `payload_as_str!` — read from `borrow<buffer>` resource
- `bad_input!` / `dependency_failed!` / `processing_failed!` / `unrecoverable!` — error constructors
- `define_state!` / `set_state!` / `with_state!` — safe state management
- `log_info!` / `log_warn!` / `log_error!` — host logging import

`parse_config<T>` is the one regular function (uses only `&str` and `String`, no WIT types), gated behind `features = ["serde"]`.

### Decision 4: Workspace Structure

Separate `plugins/` directory with `.cargo/config.toml` targeting `wasm32-wasip2`. SDK crate (`wafer-plugin`) lives in host workspace, referenced via path dependency. Polyglot plugins in language-specific subdirectories with their own toolchains (TinyGo, componentize-py).

### Decision 5: Build Tooling

Makefile at `plugins/` root with `wasm-opt -Os` post-processing for release builds. Per-plugin `Cargo.toml` with `crate-type = ["cdylib"]`, `opt-level = "s"`, `lto = true`, `strip = "debuginfo"`. No `cargo-component` needed — `wasm32-wasip2` is tier-2 stable since Rust 1.82.

### Decision 6: Testing Strategy — Three Levels + E2E

Three-level testing pyramid:
- **Level 1:** Unit tests (native target, in plugin crate) — algorithm correctness
- **Level 2:** Plugin integration tests (`PluginTestHarness` in host) — verifies WIT boundary
- **Level 3:** Pipeline E2E tests (`TestPipeline` builder) — real runtime with mocked I/O

`TestPipeline` uses the real orchestrator/builder/runner loops but with `MemorySource` and `CollectorSink`. Covers all evaluation pipelines (A/B/C), hot-swap zero-loss, attack containment, and polyglot interop.

### Decision 7: Content Router Logic

Router reads payload via `buffer.read_all()`, parses JSON, routes on configurable field. Returns `list<port-id>`. Simple plugins use manual JSON extraction to stay dependency-free and under 10KB.

### Decision 8: Naming Convention

`wafer-{descriptive-name}` for all plugin crate names.

### Decision 9: Attack Plugins (S1–S6)

Structure and Cargo.toml created during design. All target `transform-node` world with pathological `process()` implementations (5–20 lines each).

### Decision 10: Transform DX

`output_from!(&input, payload)` — single macro preserving input identity. No builder pattern.

### Decision 11: Filter DX

Explicit `impl Guest` with `evaluate() → Result<bool, ProcessError>`. SDK provides `payload_as_str!` and `payload_bytes!`.

### Decision 12: Error Categorisation DX

Constructor macros with decision-guide documentation: `bad_input!` (DLQ, no retry), `dependency_failed!` (retry with backoff), `processing_failed!` (retry N times), `unrecoverable!` (teardown + re-instantiate). `timed_out` is generated by the host epoch interrupt, never by plugin code.

### Decision 13: wasi-nn Integration

Inference plugin targets `inference-node` world. Model loaded via `wasi:nn/graph::load_by_name()` for hot-swap support.

### Decision 14: Polyglot Plugins

Go (TinyGo) uppercase + Python (componentize-py) threshold-filter. Demo-only — not in timed experiments. Proves Component Model polyglot composition.

### Decision 15: Complex Plugin Scope & Priority

All 4 complex plugins built. EWMA anomaly-detector first (strongest hot-swap justification story), others built incrementally.

### Decision 16: Binary Size Strategy

Simple plugins avoid serde (manual JSON parsing, ~5–10KB). Complex plugins use serde (~150–300KB). All get `wasm-opt -Os` post-processing.

### Decision 17: E2E Pipeline Tests

`TestPipeline` builder using real runtime with mocked I/O. First-class testing infrastructure for correctness verification.

## Alternatives Considered

- **`OnceLock` for state** — rejected because it is read-only after init; does not work for mutable accumulators like EWMA.
- **`static mut` for state** — rejected because it requires `unsafe` at every access, is error-prone, and triggers Clippy warnings.
- **Proc macros for SDK** — rejected due to compile-time cost, separate crate requirement, and debugging difficulty. `macro_rules!` achieves the same with zero overhead and `cargo expand` introspection.
- **Regular functions for SDK** — rejected because WIT types are generated locally in each plugin by `wit_bindgen::generate!`; a separate crate cannot reference them.
- **`cargo-component` for builds** — rejected because `wasm32-wasip2` is tier-2 stable since Rust 1.82; plain `cargo build --target wasm32-wasip2` suffices with `.cargo/config.toml`.
- **Shared workspace for host + plugins** — rejected because host and plugins have different build targets; can't coexist in one Cargo workspace without per-package target overrides.
- **Builder pattern for transform output** — rejected in favour of the simpler `output_from!` macro which preserves input identity in one line.
- **State persistence across hot-swaps** — explicitly deferred to future work (`pipeline:host/kv-store` import). Current design resets state by design.

## Related RFCs

- **RFC-001** — defines the WIT contracts and 4-package structure that plugins implement.
- **RFC-003** — removes Joiner world (motivating deletion of `merge-joiner` plugin) and establishes the three active worlds (transform-node, filter-node, router-node) plus inference-node.
- **RFC-002** — defines the host-side `RuntimeEnvelope` and `borrow<buffer>` resource that `payload_bytes!` reads from.
- **RFC-005** — defines the orchestrator and hot-swap model that determines state loss semantics (new Store = fresh memory).
- **RFC-004** — defines the TOML config schema that plugins receive via `init(config)`.

## Implementation Notes

- The `wafer-plugin` crate is at `crates/wafer-plugin/`. It exports the macros described in D3 (`output_from!`, `output_with_type!`, `payload_bytes!`, `payload_as_str!`, error constructors, state macros, logging macros).
- Plugins live under `plugins/` with a top-level `Makefile` for building all plugins. Each plugin's `Cargo.toml` uses `crate-type = ["cdylib"]` and depends on `wafer-plugin` via path.
- The workspace structure slightly differs from the original decision: plugins are not in a separate Cargo workspace with their own `Cargo.toml` workspace root — instead they are listed as workspace members in the root `Cargo.toml` (pending verification). The build target is set via `.cargo/config.toml` or `mise.toml` tasks (`mise run build-plugins`).
- All evaluation plugins are implemented: pass-through, json-parse, uppercase, threshold-filter, content-router, tensor-prep, result-format, mnist-inference, cayenne-decoder, anomaly-detector, vibration-features, quality-rules.
- Attack plugins exist under `plugins/attacks/`.
- Polyglot plugins exist under `plugins/go/` and `plugins/python/`.
- `PluginTestHarness` and `TestPipeline` implementations are in `crates/wafer-core/` (test infrastructure).
