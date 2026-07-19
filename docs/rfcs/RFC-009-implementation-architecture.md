# RFC-009: Implementation Architecture — Module Structure & Crate Boundaries

- **Status:** Implemented
- **Original session date:** 2026-07-12
- **Amends:** —
- **Amended by:** —

## Abstract

This RFC defines the physical code structure that maps the eight Phase 0 design sessions into Rust workspace crates and modules. The workspace splits into seven host crates (`wafer-types`, `wafer-config`, `wafer-core`, `wafer-runtime`, `wafer-loadgen`, `waferctl`, `wafer-plugin`) with a strict one-way dependency graph, plus a separate `plugins/` workspace targeting `wasm32-wasip2`. Each crate has a clear responsibility boundary: `wafer-types` carries passive domain vocabulary with zero heavy dependencies, `wafer-config` owns parsing and DAG validation, `wafer-core` owns all runtime behavior (engine, orchestrator, runner loops, nodes, metrics, HTTP API), and `wafer-plugin` is the standalone guest SDK. This separation yields sub-2-second incremental compiles for config schema changes without rebuilding wasmtime, and enables three levels of testing (unit, plugin integration via `PluginTestHarness`, and E2E via `TestPipeline`) plus criterion benchmarks. The evaluation harness (`eval/` + `wafer-loadgen`) lives alongside the runtime for measurement without coupling to it.

## Context

WAFER's Phase 0 produced eight design sessions covering WIT contracts, host runtime, node types, config schema, orchestrator, plugin SDK, performance, and evaluation harness. Each session specified *what* the architecture should be; this document specifies *where* each piece lives in code — which crate, which module, which file.

The existing codebase (pre-refactor) had all logic in a single `wafer-core` crate with config data structures, parsing, DAG construction, wasmtime-heavy engine code, and test utilities all interleaved. This created 20+ second incremental builds for any config change (because wasmtime recompiles) and made it impossible to test config validation without linking the full runtime.

The split needed to satisfy several constraints: plugins compile for `wasm32-wasip2` (cannot share a workspace with host crates), the evaluation harness uses Python for statistical analysis (UV-managed, separate toolchain), and the thesis requires measured baselines (native Rust) using the same orchestrator infrastructure as the Wasm path to isolate the "isolation tax."

Reference repositories (Torvyn's 6-crate pattern, Spin's factors architecture, Flow-Like's AOT cache module) validated that similar-scale Wasm runtimes adopt multi-crate workspaces with strict dependency ordering.

## Decisions

### D1: Crate Dependency Graph

One-way dependency flow: `wafer-types ← wafer-config ← wafer-core ← binaries`. No cycles. `wafer-plugin` is fully independent (targets `wasm32-wasip2`, zero host dependencies).

Host workspace members: `wafer-types`, `wafer-config`, `wafer-core`, `wafer-runtime`, `wafer-loadgen`, `waferctl`.

### D2: `wafer-types` — Domain Vocabulary

Passive data only. Dependencies limited to `serde` and `thiserror`. Contains domain enums (`NodeType`, `ErrorCategory`, `OverflowPolicy`, `SimpleAction`, `NodeState`, `PipelineState`), config data structs (`Config`, `NodeDef`, `EdgeDef`, `EngineConfig`, `FuelBudgets`, `ErrorPolicyConfig`, `RetryConfig`, `SourceDef`, `SinkDef`, `WasmNodeDef`, `DeadLetterConfig`, `PipelineConfig`, `Capabilities`), runtime event types, and shared error types.

`RuntimeEnvelope` does NOT belong here (uses `bytes::Bytes`). `DagGraph` does NOT belong here (uses `petgraph`).

### D3: `wafer-config` — Parsing & Validation Logic

Dependencies: `wafer-types`, `toml`, `petgraph`, `thiserror`, `tokio` (async file read only). Never imports from `wafer-core`. Contains: `loader` (TOML → `Config`), `validation` (semantic checks with accumulated errors), `dag` (`DagGraph` with `from_config`, `topo_order`, cycle/orphan detection), `diff` (config diffing for hot-swap dispatch).

### D4: `wafer-core` — Runtime Engine

Owns all heavy dependencies (`wasmtime`, `tokio`, `bytes`, `rumqttc`, `axum`, `blake3`, `foldhash`). Internal module layout:
- `envelope.rs` — `RuntimeEnvelope`, `Arc<EnvelopeHeader>`, `Lineage`
- `engine/` — `WaferEngine`, three bindgen modules, `WaferBuffer`, `ComponentCache`, `WaferState`
- `node/` — Traits (`Transform`, `Filter`, `Router`, `Source`, `Sink`), Wasm impls, state tracker, source/sink subdirectories
- `runner/` — Per-type loops (transform, filter, router, source, sink), `ErrorPolicyExecutor`, `RetryBuffer`
- `orchestrator/` — `PipelineOrchestrator`, builder, watch-channel hot-swap
- `metrics.rs` — Always-on `NodeMetrics` (atomics) + optional Prometheus exposition
- `registry.rs` — OCI pull + AOT disk cache
- `api/` — HTTP control plane (feature-gated: `http-api`)

### D5: `wafer-plugin` — Guest SDK

Standalone, targets `wasm32-wasip2`. Dependencies: `serde` + `serde_json` (feature-gated). Provides `macro_rules!` macros for output construction, error creation, state management (`thread_local!` + `RefCell`), logging, and a `parse_config<T>()` function.

Macros (not functions) because WIT types are generated locally per-plugin by `wit_bindgen::generate!` and a separate crate cannot reference them.

### D6: Per-Type Runner Loops

No generic `run_node_loop` with a match. Each node category gets its own file (`transform.rs`, `filter.rs`, `router.rs`, `source.rs`, `sink.rs`) optimized for its semantics (ownership vs borrow, 1:1 vs fan-out).

### D7: Feature Gate Only Exposition

`NodeMetrics` atomics are always compiled. Only the Prometheus HTTP endpoint is behind `#[cfg(feature = "http-api")]`.

### D8: Separate Plugin Workspace

Plugins compile to `wasm32-wasip2` — they cannot be in the same Cargo workspace as host crates. Structure: `plugins/Cargo.toml` (workspace), `.cargo/config.toml` (target override), `Makefile` (build/release/clean), `wit/` (4-package WIT, source of truth), individual plugin crates, plus `attacks/`, `go/`, and `python/` directories for evaluation and polyglot scenarios.

### D9: Evaluation Harness Layout

`eval/` directory alongside the runtime: `configs/` (experiment pipeline TOMLs), `loadgen/` (rate profiles), `scripts/` (RPi setup, automation), `analysis/` (UV-managed Python with notebooks and `wafer_analysis` package), `results/` (gitignored raw data), `Makefile`, `README.md`.

`wafer-loadgen` is a separate binary crate for external MQTT load generation with constant arrival rate and `intended_publish_ns` timestamps to prevent coordinated omission.

### D10: Testing Pyramid

- Level 1: Unit tests inside each plugin crate (native target, pure logic).
- Level 2: `PluginTestHarness` in `wafer-core` — real wasmtime, single-call verification.
- Level 3: `TestPipeline` E2E — full orchestrator with `MemorySource` + `CollectorSink`.
- Level 4: Criterion benchmarks — `BenchSource` + `BenchSink` for per-hop latency, throughput, hot-swap timing.

### D11: Native Rust Baseline

`NativeTransform` in `wafer-core` uses the same orchestrator + channels but calls native Rust functions instead of Wasm. Three layers: Layer 0 (inline, no channels), Layer 1 (native with channels), Layer 2 (full Wasm). Measures isolation tax.

### D12: File Deletions

Specified files from the pre-refactor codebase to delete: `queue/bounded.rs`, `queue/` module, `node/joiner.rs`, `orchestrator/routing.rs`, `runner/result_handler.rs`, `runner/metrics_helper.rs`, `runner/dlq_handlers.rs`, `runner/sink_helpers.rs`, `runner/overflow.rs`, `config/` (moved to `wafer-config`), `dag/` (moved to `wafer-config`), `dlq/mod.rs`, and `plugins/merge-joiner/`.

### D13: Placement Rules

When implementing a new piece: domain enums/config structs → `wafer-types`; parsing/validation logic → `wafer-config`; anything touching wasmtime → `wafer-core/engine/`; runtime data structures → `wafer-core/envelope.rs`; node traits/impls → `wafer-core/node/`; I/O connectors → `wafer-core/node/source/` or `sink/`; per-message execution loops → `wafer-core/runner/`; pipeline lifecycle → `wafer-core/orchestrator/`; guest helpers → `wafer-plugin`; HTTP endpoints → `wafer-core/api/`; benchmark adapters → source/sink modules.

## Alternatives Considered

- **Monolithic `wafer-core` (status quo):** Rejected because any config struct change triggered a full wasmtime rebuild (~20s). The split cuts incremental builds to ~2s for the config path.

- **Config structs in `wafer-config` instead of `wafer-types`:** Rejected because both `wafer-config` (for parsing) and `wafer-core` (for building the orchestrator) need the same `Config` struct. A shared vocabulary crate (`wafer-types`) avoids circular dependencies.

- **Shared `wafer-testing` crate for test utilities:** Not adopted; `PluginTestHarness` and `TestPipeline` live in `wafer-core` because they use its internal types directly. Extracting them would require re-exporting engine internals.

- **Single `runner/loop.rs` with generic dispatch:** Rejected (D6). Per-type files allow ownership-model specialization (transforms take ownership, filters/routers borrow) without branching at runtime.

- **Plugins in the same workspace with conditional compilation:** Impossible — `wasm32-wasip2` target is incompatible with host targets in a single Cargo workspace resolution.

## Related RFCs

- **RFC-001** — Defines the 4-package WIT contract structure that D4's `engine/bindings.rs` implements.
- **RFC-002** — Specifies `RuntimeEnvelope`, `WaferState`, and buffer lifecycle mapped to `engine/state.rs` and `envelope.rs`.
- **RFC-003** — Specifies the node types and trait signatures in `node/traits.rs`; confirms Joiner removal and merge-as-topology.
- **RFC-004** — Specifies the config schema that lives in `wafer-types` and the validation logic in `wafer-config`.
- **RFC-005** — Specifies the orchestrator, builder, and watch-channel hot-swap mapped to `orchestrator/`.
- **RFC-006** — Specifies the guest SDK design for `wafer-plugin` and the plugin workspace layout.
- **RFC-007** — Specifies the AOT cache, epoch ticker, and metering in `engine/cache.rs` and `engine/mod.rs`.
- **RFC-008** — Specifies the evaluation harness design mapped to `eval/`, `wafer-loadgen`, `BenchSource`, and `BenchSink`.

## Implementation Notes

Code matches decisions; no divergence. The workspace structure is fully implemented as specified:

- `crates/wafer-types/` contains all config data structs and domain enums per D2.
- `crates/wafer-config/` contains `loader.rs`, `validation.rs`, `dag.rs`, `error.rs` per D3.
- `crates/wafer-core/src/` matches the internal module layout specified in D4 (engine/, node/, runner/, orchestrator/, envelope.rs, metrics.rs, registry.rs, api/).
- `crates/wafer-plugin/` is a standalone SDK crate targeting `wasm32-wasip2` per D5.
- `plugins/` is a separate workspace with `.cargo/config.toml` targeting `wasm32-wasip2` per D8.
- `eval/` contains configs, scripts, and a UV-managed Python analysis package per D9.
- `crates/wafer-loadgen/` provides the external MQTT load generator per D9.
- Per-type runner loops are separate files (`runner/transform.rs`, `runner/filter.rs`, `runner/router.rs`, `runner/source.rs`, `runner/sink.rs`) per D6.
- The deleted files listed in D12 no longer exist in the current codebase.
