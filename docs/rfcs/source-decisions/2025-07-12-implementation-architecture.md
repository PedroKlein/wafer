# Implementation Architecture — Module Structure & Crate Boundaries

**Date:** 2025-07-12  
**Status:** Decided  
**Location:** `docs/decisions/2025-07-12-implementation-architecture.md` (canonical)  
**Scope:** Crate organization, module layout, dependency boundaries for implementation  
**Depends on:** All 8 Phase 0 decision documents  
**Workflows:** `docs/workflows/` (planning-session.md, implementation-session.md)  
**Feeds into:** Implementation task graph  

---

## Purpose

This document defines the physical code structure that realizes the 8 Phase 0 design sessions. It answers: "where does each piece of the target architecture live in code?"

When implementing any task, use this document to find:
1. **Which module** to create/modify (module layout section)
2. **Which decision document** has the full specification (reference table below)
3. **Which current files** are being replaced (deletion table)

---

## Decision Documents (Full Paths)

These contain the complete specification for each architectural area. Read the relevant one(s) before implementing any task.

| Session | File Path | Key Content |
|---|---|---|
| 1: WIT Contracts | `docs/decisions/2025-07-05-wit-contracts-envelope-design.md` | WIT packages, worlds, types, buffer resource, process-error, filter interface |
| 2: Host Runtime | `docs/decisions/2025-07-06-host-runtime-architecture.md` | WaferState, ResourceTable, Bytes payload, InstancePre, Store lifecycle, error policy |
| 3: Node Types | `docs/decisions/2025-07-06-node-type-architecture.md` | 4 node types, AnyNode struct, trait signatures, borrow vs ownership, merge, fuel |
| 4: Config Schema | `docs/decisions/2025-07-06-config-schema-pipeline-ux.md` | TOML schema, map-keyed nodes, validation, defaults, error policy cascade |
| 5: Orchestrator | `docs/decisions/2025-07-12-orchestrator-runtime-simplification.md` | Builder, runner loops, watch-channel hot-swap, ErrorPolicyExecutor, shutdown |
| 6: Plugins & SDK | `docs/decisions/2025-07-12-plugin-rewrite-guest-sdk.md` | 20 plugins, guest SDK, workspace, testing pyramid, complex plugin specs |
| 7: Performance | `docs/decisions/2025-07-12-performance-optimizations.md` | AOT cache, metering flags, StoreLimits, epoch OS thread, code quality (C1-C4) |
| 8: Evaluation | `docs/decisions/2025-07-12-evaluation-harness-design.md` | BenchSource/Sink, wafer-loadgen, HdrHistogram, native baseline, Python analysis |
| Plan Index | `docs/decisions/PHASE-0-PLAN.md` | Session index, cross-references, amendments summary |
| This Document | `docs/decisions/2025-07-12-implementation-architecture.md` | Module structure, crate boundaries, implementation mapping |

## Current Source Files (Being Replaced)

When implementing, read the CURRENT file to understand what exists, then the decision document for the TARGET.

| Current File | Disposition | Specified By |
|---|---|---|
| `wit/transform.wit` | REWRITE | Session 1 (full WIT spec) |
| `wit/router.wit` | REWRITE | Session 1 |
| `wit/joiner.wit` | DELETE | Session 3 A1 |
| `wit/lifecycle.wit` | REWRITE | Session 1 |
| `wit/types.wit` | REWRITE | Session 1 |
| `crates/wafer-core/src/queue/envelope.rs` | REWRITE | Sessions 2+3 (RuntimeEnvelope redesign) |
| `crates/wafer-core/src/queue/bounded.rs` | DELETE | Session 5 |
| `crates/wafer-core/src/engine/host.rs` | REFACTOR → `state.rs` | Session 2 D8, Session 7 D9 |
| `crates/wafer-core/src/engine/instance.rs` | REWRITE → `bindings.rs` | Session 3 D6 |
| `crates/wafer-core/src/engine/loader.rs` | REFACTOR | Session 7 C1 (OS thread), D1 (AOT cache) |
| `crates/wafer-core/src/orchestrator/pipeline.rs` | REWRITE | Session 5 D6, D12 |
| `crates/wafer-core/src/orchestrator/builder.rs` | REWRITE | Session 5 D2 |
| `crates/wafer-core/src/orchestrator/routing.rs` | DELETE | Session 5 (amendment) |
| `crates/wafer-core/src/orchestrator/hotswap.rs` | REWRITE | Session 5 D6 |
| `crates/wafer-core/src/runner/loops.rs` | REWRITE → split per-type | Session 5 D3 |
| `crates/wafer-core/src/runner/result_handler.rs` | DELETE | Session 5 D3 |
| `crates/wafer-core/src/runner/metrics_helper.rs` | DELETE | Session 5 D10 |
| `crates/wafer-core/src/node/mod.rs` | REWRITE | Session 3 D2 (AnyNode struct) |
| `crates/wafer-core/src/node/traits.rs` | REWRITE | Session 3 D7, D8 |
| `crates/wafer-core/src/node/transform.rs` | REFACTOR | Sessions 2+3 |
| `crates/wafer-core/src/node/router.rs` | REFACTOR | Session 3 |
| `crates/wafer-core/src/node/joiner.rs` | DELETE | Session 3 A1 |
| `crates/wafer-core/src/node/state.rs` | REFACTOR | Session 3 D5, Session 7 C4 |
| `crates/wafer-core/src/node/source/mqtt.rs` | REFACTOR | Session 2 D1 (Bytes) |
| `crates/wafer-core/src/config/schema.rs` | REWRITE → `wafer-types` | Session 4 |
| `crates/wafer-core/src/config/loader.rs` | MOVE → `wafer-config` | Session 4 |
| `crates/wafer-core/src/config/diff.rs` | MOVE → `wafer-config` | Session 5 D9 |
| `crates/wafer-core/src/dag/graph.rs` | MOVE → `wafer-config` | Session 4 |
| `crates/wafer-core/src/metrics/` | REWRITE | Session 5 D10, Session 7 |
| `crates/wafer-core/src/dlq/mod.rs` | ABSORB → `error_policy.rs` | Session 5 D8 |
| `plugins/pass-through/src/lib.rs` | REWRITE | Session 6 |
| `plugins/uppercase/src/lib.rs` | REWRITE | Session 6 |
| `plugins/filter/src/lib.rs` | REWRITE → `threshold-filter` | Session 6 |
| `plugins/content-router/src/lib.rs` | REWRITE | Session 6 |
| `plugins/merge-joiner/src/lib.rs` | DELETE | Session 3 A1 |

## Reference Repositories (for patterns, read via file paths)

| Repo | Path | Relevant Patterns |
|---|---|---|
| Torvyn (closest sibling) | `~/Dev/pi-repos/repos/github.com/torvyn/torvyn/main` | Pipeline builder, WIT bindings, AOT cache, buffer pool, config |
| Flow-Like (AOT cache) | `~/Dev/pi-repos/repos/github.com/Rheosoph/flow-like/dev` | `packages/wasm/src/aot_cache.rs`, `engine.rs` |
| wasmtime (runtime internals) | `~/Dev/pi-repos/repos/github.com/bytecodealliance/wasmtime/main` | ResourceTable API, Store, InstancePre, fuel |
| wit-bindgen (plugin codegen) | `~/Dev/pi-repos/repos/github.com/bytecodealliance/wit-bindgen/main` | `generate!` macro usage, guest patterns |
| Spin (Component Model lifecycle) | `~/Dev/pi-repos/repos/github.com/fermyon/spin/main` | `crates/core/src/store.rs`, epoch handling |
| tremor-rs (DAG backpressure) | `~/Dev/pi-repos/repos/github.com/tremor-rs/tremor-runtime/main` | Contraflow, bench connector pattern |
| eKuiper (comparison target) | `~/Dev/pi-repos/repos/github.com/lf-edge/ekuiper/master` | `internal/topo/node/`, SQL rules |
| tcc-doc (thesis research) | `~/Dev/github.com/PedroKlein/tcc-doc/main` | `findings/`, `research/analysis/evaluation-plan.md` |

---

## Crate Dependency Graph

```
wafer-types (pure domain vocabulary, zero heavy deps)
     ↑               ↑
     │               │
wafer-config     wafer-core ──────→ wafer-runtime (binary)
(parsing +       (engine +          waferctl (CLI)
 validation)      orchestrator +    wafer-loadgen (MQTT load gen binary)
                  runner + nodes)

wafer-plugin (standalone, wasm32-wasip2 target, zero host deps)
```

**One-way flow:** `types ← config ← core ← binaries`  
**No cycles.** `wafer-types` is the foundation. `wafer-plugin` is fully independent.

**Host workspace members:** `wafer-types`, `wafer-config`, `wafer-core`, `wafer-runtime`, `wafer-loadgen`, `waferctl`  
**Separate workspace:** `plugins/` (wasm32-wasip2 target)  
**Non-Rust:** `eval/analysis/` (UV-managed Python)

---

## Crate Responsibilities

### `wafer-types` — Domain Vocabulary

**Rule:** Passive data only. No methods that reference heavy deps. No I/O.  
**Deps:** `serde`, `thiserror` only.

Contains:
- **Domain enums:** `NodeType`, `ErrorCategory`, `OverflowPolicy`, `SimpleAction`, `NodeState`, `PipelineState`
- **Config data structs:** `Config`, `NodeDef`, `EdgeDef`, `EngineConfig`, `FuelBudgets`, `ErrorPolicyConfig`, `RetryConfig`, `SourceDef`, `SinkDef`, `WasmNodeDef`, `DeadLetterConfig`, `PipelineConfig`, `Capabilities`
- **Runtime event types:** `PipelineEvent`, `ControlMessage`
- **Shared error types:** `ConfigError`, `ValidationError`

**Why config DATA lives here (not parsing LOGIC):**
- `Config` is a serde struct — any crate can deserialize into it
- `wafer-core` uses config types directly in builder/orchestrator
- `wafer-config` uses them as the target of parsing
- Both depend on wafer-types → shared vocabulary, single source of truth

**What does NOT belong here:**
- `RuntimeEnvelope` (uses `bytes::Bytes` — too heavy)
- `DagGraph` (uses `petgraph` — belongs in wafer-config)
- Anything with wasmtime, tokio, or I/O deps

### `wafer-config` — Parsing & Validation Logic

**Rule:** Functions that produce/validate typed config. No runtime behavior.  
**Deps:** `wafer-types`, `toml`, `petgraph`, `thiserror`, `tokio` (async file read only)

Contains:
- **`loader`** — `load_config(path) -> Result<Config>` (read TOML, deserialize)
- **`validation`** — `validate(&Config) -> Result<(), Vec<ValidationError>>` (semantic checks, accumulated errors)
- **`dag`** — `DagGraph` struct (petgraph wrapper): `from_config()`, `topo_order()`, `contains_node()`, cycle detection, merge validation, orphan detection
- **`diff`** — `diff_configs(old, new) -> ConfigDiff` (plugin vs config-only changes)

**Public interface to wafer-core:**
```rust
pub fn load_config(path: &Path) -> Result<Config>;
pub fn validate(config: &Config) -> Result<(), Vec<ValidationError>>;
pub struct DagGraph { /* opaque internals */ }
impl DagGraph {
    pub fn from_config(config: &Config) -> Result<Self>;
    pub fn topo_order(&self) -> &[String];
    pub fn contains_node(&self, id: &str) -> bool;
    pub fn node_count(&self) -> usize;
    pub fn edge_count(&self) -> usize;
}
pub fn diff_configs(old: &Config, new: &Config) -> ConfigDiff;
```

**Compile-time benefit:** Changes to config schema (frequent during development) recompile only wafer-types + wafer-config (~2s), NOT wasmtime (~20s).

### `wafer-core` — Runtime Engine

**Rule:** Everything that runs the pipeline. Owns heavy deps.  
**Deps:** `wafer-types`, `wafer-config`, `wasmtime`, `tokio`, `bytes`, `rumqttc`, `hyper`, `reqwest`, `blake3`, `foldhash`

Contains (see Module Layout below for full structure):
- **`envelope`** — `RuntimeEnvelope`, `Arc<EnvelopeHeader>`, `Lineage`
- **`engine/`** — `WaferEngine`, 3 bindgen modules, `WaferBuffer`, `ComponentCache`, `WaferState`
- **`node/`** — Traits (`Transform`, `Filter`, `Router`, `Source`, `Sink`), Wasm impls, state tracker
- **`runner/`** — Per-type loops, `ErrorPolicyExecutor`, `RetryBuffer`, `NodeMetrics`
- **`orchestrator/`** — `PipelineOrchestrator`, builder, watch-channel hot-swap
- **`metrics`** — Unconditional `NodeMetrics` (atomics) + optional Prometheus exposition
- **`registry`** — OCI pull + AOT disk cache
- **`api/`** — HTTP control plane (feature-gated: `http-api`)

### `wafer-plugin` — Guest SDK

**Rule:** Fully standalone. Targets wasm32-wasip2. Zero host deps.  
**Deps:** `serde` + `serde_json` (behind `features = ["serde"]`)

Contains:
- `macro_rules!` macros: `output_from!`, `payload_bytes!`, `payload_as_str!`, error constructors (`bad_input!`, `dependency_failed!`, etc.), state management (`define_state!`, `set_state!`, `with_state!`), logging (`log_info!`, `log_warn!`, `log_error!`)
- `parse_config<T>(json: &str) -> Result<T, String>` (regular function, feature-gated)

### `wafer-runtime` — Binary Entry Point

**Deps:** `wafer-core`, `wafer-config`, `wafer-types`, `tokio`, `clap`, `tracing-subscriber`, `anyhow`

Thin main: parse CLI args → load config → build orchestrator → run → handle signals.

### `waferctl` — CLI Tool

**Deps:** `clap`, `reqwest`, `serde_json`, `tabled`, `wafer-types` (for response deserialization)

Communicates with wafer-runtime via HTTP API. No direct Rust coupling to internals.

---

## `wafer-core` Internal Module Layout

```
crates/wafer-core/src/
├── lib.rs                     # Public API: re-exports key types
├── error.rs                   # WaferError, RegistryError (thiserror)
├── envelope.rs                # RuntimeEnvelope, Arc<EnvelopeHeader>, Lineage
│
├── engine/
│   ├── mod.rs                 # WaferEngine: creation, OS-thread epoch ticker, shutdown
│   ├── bindings.rs            # 3 bindgen! modules (transform/filter/router) + WasmBindings enum
│   ├── buffer.rs              # WaferBuffer resource (HostBuffer trait impl for borrow<buffer>)
│   ├── cache.rs               # ComponentCache: blake3-keyed, two-tier (memory HashMap + disk .cwasm)
│   └── state.rs               # WaferState: WasiCtx, ResourceTable, StoreLimits, log_buffer, node_id
│
├── node/
│   ├── mod.rs                 # AnyNode { id: Box<str>, state_tracker: Arc<NodeStateTracker>, kind: NodeKind }
│   ├── traits.rs              # Transform, Filter, Router (async fn in trait), Source, Sink (Pin<Box>)
│   ├── wasm.rs                # WasmTransform, WasmFilter, WasmRouter (share WaferEngine + InstancePre)
│   ├── state.rs               # NodeStateTracker + ProcessingGuard (RAII Drop)
│   ├── source/
│   │   ├── mod.rs             # Source trait re-export, BenchSource (in-process load gen)
│   │   ├── mqtt.rs            # MqttSource (rumqttc, produces Bytes payload)
│   │   ├── file.rs            # FileSource (line-by-line)
│   │   ├── stdin.rs           # StdinSource
│   │   └── http.rs            # HttpSource (webhook receiver)
│   └── sink/
│       ├── mod.rs             # Sink trait re-export, CollectorSink (test), NullSink, BenchSink
│       ├── mqtt.rs            # MqttSink
│       ├── file.rs            # FileSink
│       ├── stdout.rs          # StdoutSink
│       ├── http.rs            # HttpSink
│       └── batch.rs           # BatchingSink trait + flush timer pattern
│
├── runner/
│   ├── mod.rs                 # Shared: fan_out helper, send_downstream
│   ├── transform.rs           # run_transform_loop (ownership, DLQ safety clone, watch check)
│   ├── filter.rs              # run_filter_loop (borrow, zero-copy forward)
│   ├── router.rs              # run_router_loop (borrow, fan-out, last-port-move)
│   ├── source.rs              # run_source_loop (poll + cancel)
│   ├── sink.rs                # run_sink_loop (batch timer, flush on shutdown)
│   └── error_policy.rs        # ErrorPolicyExecutor, RetryBuffer, DlqEnvelope, DlqReason
│
├── orchestrator/
│   ├── mod.rs                 # PipelineOrchestrator (build → spawn → watch → teardown)
│   ├── builder.rs             # from_config: node construction, receiver-keyed queue wiring, policy resolution
│   └── hotswap.rs             # watch::channel SwapPayload, config diff dispatch, SwapTimeline
│
├── metrics.rs                 # NodeMetrics (always-on AtomicU64), optional PrometheusExporter
├── registry.rs                # OCI client + docker credentials (flat file or registry/ if > 300 lines)
│
└── api/                       # Feature-gated: http-api
    ├── mod.rs
    ├── server.rs              # axum server setup
    ├── handlers.rs            # REST endpoints (status, hot-swap, resync, metrics)
    └── metrics.rs             # Prometheus scrape handler
```

---

## Mapping: Decision Documents → Modules

| Decision Document | Primary Modules Affected |
|---|---|
| Session 1: WIT Contracts | `plugins/wit/`, `engine/bindings.rs`, `engine/buffer.rs` |
| Session 2: Host Runtime | `engine/state.rs`, `envelope.rs`, `engine/buffer.rs` |
| Session 3: Node Types | `node/mod.rs`, `node/traits.rs`, `node/wasm.rs`, `node/state.rs` |
| Session 4: Config Schema | `wafer-types/` (config structs), `wafer-config/` (parsing + validation) |
| Session 5: Orchestrator | `orchestrator/`, `runner/`, `runner/error_policy.rs` |
| Session 6: Plugins & SDK | `wafer-plugin/`, `plugins/` workspace |
| Session 7: Performance | `engine/cache.rs`, `engine/mod.rs` (OS thread), `metrics.rs`, `envelope.rs` (Box<str>) |
| Session 8: Evaluation | `node/source/mod.rs` (BenchSource), `node/sink/mod.rs` (BenchSink), `eval/` |

---

## What's Deleted (vs Current Codebase)

| Current File/Module | Action | Reason |
|---|---|---|
| `queue/bounded.rs` | DELETE | Session 5: use raw `tokio::mpsc` directly |
| `queue/` module | DELETE | `envelope.rs` promoted to crate root |
| `node/joiner.rs` | DELETE | Session 3 A1: Joiner removed, merge is topology |
| `orchestrator/routing.rs` | DELETE | Session 5: `RoutingController` removed |
| `runner/result_handler.rs` | DELETE | Per-type loops handle their own dispatch |
| `runner/metrics_helper.rs` | DELETE | Replaced by `NodeMetrics` unconditional atomics |
| `runner/dlq_handlers.rs` | ABSORB | Logic moves into `runner/error_policy.rs` |
| `runner/sink_helpers.rs` | ABSORB | Logic moves into `runner/sink.rs` |
| `runner/overflow.rs` | ABSORB | Logic moves into `runner/mod.rs` (shared send helper) |
| `config/` (in wafer-core) | MOVE | Splits into `wafer-types` (structs) + `wafer-config` (logic) |
| `dag/` (in wafer-core) | MOVE | Moves to `wafer-config/src/dag.rs` |
| `dlq/mod.rs` | ABSORB | `DlqEnvelope` struct moves to `runner/error_policy.rs` |

---

## Placement Rules (For Implementation)

When implementing a new piece, use these rules to decide where it goes:

| If it's... | Put it in... | Example |
|---|---|---|
| A domain enum or config struct (serde-derivable, no heavy deps) | `wafer-types` | `ErrorCategory`, `NodeType`, `Config` |
| Logic that parses/validates config files | `wafer-config` | `load_config()`, DAG cycle detection |
| Anything that touches wasmtime | `wafer-core/engine/` | `WaferEngine`, `ComponentCache` |
| A runtime data structure flowing through the pipeline | `wafer-core/envelope.rs` | `RuntimeEnvelope` |
| A node trait or its Wasm implementation | `wafer-core/node/` | `Transform` trait, `WasmFilter` |
| An I/O connector (network, file, stdin) | `wafer-core/node/source/` or `sink/` | `MqttSource` |
| A per-message execution loop | `wafer-core/runner/` | `run_transform_loop` |
| Pipeline lifecycle management | `wafer-core/orchestrator/` | `PipelineOrchestrator` |
| A guest-side helper for plugin authors | `wafer-plugin` | `output_from!` macro |
| An HTTP endpoint | `wafer-core/api/` (feature-gated) | `/nodes/:id/swap` handler |
| A benchmark/evaluation adapter | `wafer-core/node/source/mod.rs` or `sink/mod.rs` | `BenchSource`, `BenchSink` |

---

## Key Architectural Invariants

These are non-negotiable properties that the module structure enforces:

1. **`wafer-types` has zero heavy deps.** If you find yourself importing `wasmtime`, `tokio`, `bytes`, or `petgraph` in wafer-types, you're putting code in the wrong place.

2. **`wafer-config` never imports from `wafer-core`.** The dependency is strictly one-way. Config parsing cannot depend on runtime behavior.

3. **`wafer-plugin` is completely standalone.** It compiles for wasm32-wasip2. It CANNOT reference any host crate.

4. **`RuntimeEnvelope` is NOT in wafer-types.** It uses `bytes::Bytes` and `Arc` — these are runtime optimization choices that shouldn't leak to the types layer.

5. **Behavior lives in the crate that owns the deps.** `EngineConfig` (data) is in wafer-types. `WaferEngine::from_config(&EngineConfig)` (behavior using wasmtime) is in wafer-core.

6. **Per-type runner loops are separate files.** No generic `run_node_loop` with a match. Each loop is optimized for its node category (ownership vs borrow, 1:1 vs fan-out).

7. **Feature gate only exposition, not measurement.** `NodeMetrics` atomics are always compiled. Only the Prometheus HTTP endpoint is behind `#[cfg(feature = "http-api")]`.

---

## Testing Strategy by Crate

| Crate | Test Approach | What It Validates |
|---|---|---|
| `wafer-types` | Unit tests (serde round-trips, enum coverage) | Serialization correctness |
| `wafer-config` | Unit tests (valid/invalid TOML, validation rules, DAG properties) | Config parsing, validation completeness |
| `wafer-core` | Integration tests (real Wasm plugins, PluginTestHarness, TestPipeline) | End-to-end runtime correctness |
| `wafer-plugin` | Unit tests (native target, no Wasm) | SDK macro expansion, parse_config |
| `wafer-core` (eval) | Benchmark tests (criterion, BenchSource/BenchSink) | Performance measurement |

**Key benefit of split:** Config tests run in <1s. You can iterate on schema design without waiting for wasmtime to compile.

---

---

## Plugin Workspace (Separate from Host)

Plugins compile to `wasm32-wasip2` — they CANNOT be in the same Cargo workspace as host crates (different target). Session 6 D4 defines:

```
plugins/
├── Cargo.toml                  # Workspace: all Rust plugins
├── .cargo/config.toml          # [build] target = "wasm32-wasip2"
├── Makefile                    # build, release (wasm-opt -Os), clean
├── wit/                        # 4-package WIT (source of truth)
│   ├── pipeline-types.wit
│   ├── pipeline-node.wit
│   ├── pipeline-routing.wit
│   ├── pipeline-host.wit
│   └── deps/wasi-nn/
│
├── pass-through/               # wafer-pass-through (transform, ~5KB)
├── json-parse/                 # wafer-json-parse (transform, ~8KB)
├── uppercase/                  # wafer-uppercase (transform, ~5KB)
├── threshold-filter/           # wafer-threshold-filter (filter, ~8KB)
├── content-router/             # wafer-content-router (router, ~8KB)
├── tensor-prep/                # wafer-tensor-prep (transform, ~6KB)
├── result-format/              # wafer-result-format (transform, ~8KB)
├── mnist-inference/            # wafer-mnist-inference (inference, ~20KB)
├── cayenne-decoder/            # wafer-cayenne-decoder (transform, ~10KB)
├── anomaly-detector/           # wafer-anomaly-detector (transform, ~160KB)
├── vibration-features/         # wafer-vibration-features (transform, ~300KB)
├── quality-rules/              # wafer-quality-rules (filter, ~150KB)
│
├── attacks/                    # S1–S6 attack scenario plugins
│   ├── buffer-overflow/
│   ├── infinite-loop/
│   ├── memory-exhaust/
│   ├── fs-access/
│   └── panic/
│
├── go/                         # Polyglot: TinyGo
│   └── uppercase/
│       ├── main.go
│       ├── go.mod
│       └── Makefile
│
└── python/                     # Polyglot: componentize-py
    └── threshold-filter/
        ├── app.py
        └── Makefile
```

**Build rules:**
- `cd plugins && cargo build` → debug builds for all Rust plugins
- `cd plugins && make release` → optimized + wasm-opt post-processing
- Each plugin's `Cargo.toml`: `crate-type = ["cdylib"]`, depends on `wafer-plugin` via path
- `wit_bindgen::generate!({ path: "../wit", world: "transform-node" })` in each lib.rs

**Plugin categories (Session 6 D1):**
| Category | Plugins | Purpose |
|---|---|---|
| Evaluation (simple) | pass-through, json-parse, uppercase, threshold-filter, content-router, tensor-prep, result-format, mnist-inference | Benchmarks, eKuiper comparison |
| Complex (justify architecture) | cayenne-decoder, anomaly-detector, vibration-features, quality-rules | Demonstrate WHY Wasm isolation + hot-swap matter |
| Polyglot | uppercase-go, threshold-filter-py | Prove Component Model language-agnostic composition |
| Attack scenarios | S1–S6 | RQ2 fault containment verification |

---

## Evaluation Harness (`eval/` + `wafer-loadgen`)

Session 8 defines the complete measurement infrastructure. This lives alongside the runtime, not inside it.

### `wafer-loadgen` Binary Crate

```
crates/wafer-loadgen/           # Separate binary for external MQTT load generation
├── Cargo.toml                  # deps: rumqttc, tokio, clap, serde_json
└── src/main.rs                 # RateController + MQTT publish at constant arrival rate
```

**Purpose:** External load generator publishing to MQTT at constant rate with `intended_publish_ns` timestamps. Prevents coordinated omission (Tene 2012). Used for E2E experiments where SUT and load generator must be separate processes.

### `eval/` Directory Structure

```
eval/
├── configs/                     # Pipeline TOML configs for each experiment
│   ├── pipeline-a-telemetry.toml
│   ├── pipeline-b-inference.toml
│   ├── pipeline-c-passthrough.toml
│   ├── pipeline-c-passthrough-fuel-only.toml
│   ├── pipeline-c-passthrough-epoch-only.toml
│   ├── pipeline-c-passthrough-neither.toml
│   ├── pipeline-d-native.toml
│   ├── pipeline-depth-{1,2,3,5,10}.toml
│   └── ekuiper/
│       ├── stream.json
│       └── rule.json
│
├── loadgen/                     # Load generation profiles
│   ├── steady-500.toml
│   ├── steady-1000.toml
│   ├── steady-2000.toml
│   ├── burst.toml
│   ├── ramp.toml
│   └── hotswap-trigger.toml
│
├── scripts/
│   ├── setup-rpi.sh             # CPU pinning, governor, affinity, thermal
│   ├── run-experiment.sh        # Single experiment execution
│   ├── run-all.sh               # Full evaluation suite
│   └── verify-environment.sh    # Pre-flight checks
│
├── analysis/                    # UV-managed Python project
│   ├── pyproject.toml           # UV config: scipy, numpy, matplotlib, pandas, hdrhistogram
│   ├── uv.lock                  # Pinned dependencies
│   ├── notebooks/
│   │   ├── 00-warmup-validation.ipynb
│   │   ├── 01-latency-cdf.ipynb
│   │   ├── 02-per-hop-overhead.ipynb
│   │   ├── 03-memory-scaling.ipynb
│   │   ├── 04-cross-arch.ipynb
│   │   ├── 05-hotswap-timeline.ipynb
│   │   ├── 06-fault-injection.ipynb
│   │   ├── 07-metering-decomp.ipynb
│   │   ├── 08-depth-scaling.ipynb
│   │   ├── 09-saturation.ipynb
│   │   └── 10-summary-stats.ipynb
│   └── src/wafer_analysis/
│       ├── __init__.py
│       ├── hdr_loader.py        # Load HdrHistogram interval logs
│       ├── stats.py             # Mann-Whitney U, Bootstrap CI, Cliff's Delta
│       ├── plots.py             # Shared plot styling (thesis figures)
│       └── tables.py            # LaTeX table generation
│
├── results/                     # Raw output (.gitignored, published to Zenodo)
│   └── .gitkeep
│
├── Makefile                     # make e-perf-1, make e-swap-1, make all, make figures
└── README.md                    # Complete reproduction instructions
```

**Recording output per experiment run (Rust side):**
```
experiment_run/
├── config.toml              # Exact config used
├── metadata.json            # Hardware, versions, git SHA, timestamp
├── latency.hdr              # HdrHistogram interval log
├── throughput.csv           # Periodic: timestamp, msg_count, bytes
├── per_node_metrics.csv     # node_id, messages, failures, process_ns, swap_count
├── memory.csv               # Periodic: timestamp, rss_bytes
├── swap_timeline.json       # Phase timestamps (hot-swap experiments)
└── sequence.csv             # seq_id, received_at_ns
```

**Statistical methodology (Session 8 D9):**
- N≥30 repetitions per experiment
- 30s warmup exclusion + ADF stationarity verification
- Mann-Whitney U test (non-parametric comparison)
- Bootstrap 95% CI (10,000 resamples)
- Cliff's Delta (effect size)
- HdrHistogram: 3 significant digits, 1µs–10s range

---

## Testing Infrastructure

Three-level testing pyramid (Session 6 D6) + evaluation-grade benchmarks (Session 8).

### Level 1: Unit Tests (native target, per-plugin)

**Location:** Inside each plugin crate (`plugins/*/src/lib.rs` `#[cfg(test)]` modules)  
**Target:** `x86_64-unknown-linux-gnu` (NOT wasm32)  
**Scope:** Pure logic — EWMA algorithm, JSON field extraction, routing rules  
**Run:** `cd plugins && cargo test --target x86_64-unknown-linux-gnu`

### Level 2: Plugin Integration Tests (`PluginTestHarness`)

**Location:** `crates/wafer-core/src/testing.rs` (public module for test consumers)  
**Scope:** Plugin loaded into real wasmtime, WIT boundary exercised, single-call verification

```rust
pub struct PluginTestHarness { engine: WaferEngine }

impl PluginTestHarness {
    pub fn new() -> Self;
    pub async fn transform(wasm_path: &str, config: &str) -> Result<TransformTestInstance>;
    pub async fn filter(wasm_path: &str, config: &str) -> Result<FilterTestInstance>;
    pub async fn router(wasm_path: &str, config: &str) -> Result<RouterTestInstance>;
}

impl TransformTestInstance {
    pub async fn process(&mut self, payload: &[u8]) -> Result<Vec<u8>>;
    pub async fn process_message(&mut self, msg: TestMessage) -> Result<TestOutput>;
    pub async fn process_expect_error(&mut self, payload: &[u8]) -> Result<WasmProcessError>;
}

impl FilterTestInstance {
    pub async fn evaluate(&mut self, payload: &[u8]) -> Result<bool>;
}

impl RouterTestInstance {
    pub async fn route(&mut self, payload: &[u8]) -> Result<Vec<String>>;
    pub fn output_ports(&self) -> &[String];
}
```

**Run:** `cargo test -p wafer-core --test plugin_integration`

### Level 3: E2E Pipeline Tests (`TestPipeline`)

**Location:** `crates/wafer-core/tests/` (integration test files)  
**Scope:** Full pipeline with real orchestrator, builder, runner loops — only I/O is mocked

```rust
// Uses real runtime with MemorySource + CollectorSink
let outputs = TestPipeline::new()
    .source(vec![json!({...}), json!({...})])
    .transform("parse", "plugins/wafer_json_parse.wasm", "")
    .filter("threshold", "plugins/wafer_threshold_filter.wasm", config)
    .router("router", "plugins/wafer_content_router.wasm", config)
    .sink("alert", port: "alert")
    .sink("log", port: "log")
    .run()
    .await
    .unwrap();

assert_eq!(outputs["alert"].len(), expected_alerts);
```

**Key design (Session 8 D15):** `TestPipeline` is a convenience wrapper around the REAL `PipelineBuilder` with pluggable source/sink adapters:

| Adapter | Used By | Purpose |
|---|---|---|
| `MemorySource` | TestPipeline (correctness) | Emit from Vec, immediate |
| `BenchSource` | BenchmarkPipeline (performance) | Rate-controlled, timestamped, sequenced |
| `MqttSource` | E2E eval (production) | Real MQTT subscription |
| `CollectorSink` | TestPipeline (correctness) | Collect to `Arc<Mutex<Vec<RuntimeEnvelope>>>` |
| `BenchSink` | All benchmarks | HdrHistogram + SequenceTracker + CSV export |
| `NullSink` | Throughput saturation | /dev/null (discard) |

**E2E test coverage:**
- Pipeline A (telemetry): source → parse → filter → router → sinks
- Pipeline B (inference): source → tensor-prep → mnist → result-format → sink
- Pipeline C (passthrough): source → pass-through → sink (zero-loss verification)
- Hot-swap mid-stream: continuous source, swap at t=N, assert zero loss/duplication
- Attack containment: bad node traps, good nodes continue unaffected
- Recovery: unrecoverable → Recovering → Running (verify messages preserved)
- Merge topology: A,B → merge → Transform → Sink

**Run:** `cargo test -p wafer-core --test pipeline_e2e`

### Level 4: Criterion Benchmarks (Performance)

**Location:** `crates/wafer-core/benches/`  
**Scope:** Per-hop latency, throughput saturation, hot-swap timing

| Benchmark | Measures | Uses |
|---|---|---|
| `per_hop_latency.rs` | Wasm boundary crossing time (variable payload) | BenchSource + BenchSink |
| `throughput.rs` | Pipeline A max msg/s | BenchSource + NullSink |
| `hot_swap.rs` | Prepare/drain/flip/retire phase durations | SwapTimeline |
| `metering_overhead.rs` | 4-config comparison (fuel×epoch matrix) | BenchSource + BenchSink |
| `native_baseline.rs` | Layer 0 (inline) + Layer 1 (channels) | NativeTransform |

**Run:** `cargo bench -p wafer-core`

### Native Rust Baseline (Session 8 D5)

**Location:** `crates/wafer-core/src/node/native.rs`  
**Purpose:** Same orchestrator + same channels + native Rust functions (no Wasm). Measures isolation tax.

```rust
pub trait ProcessNode: Send + 'static {
    async fn process(&mut self, envelope: RuntimeEnvelope) -> Result<RuntimeEnvelope, ProcessError>;
}

// Wasm implementation:
impl ProcessNode for WasmTransform { /* fuel reset, WIT marshal, Wasm call */ }

// Native implementation (baseline):
pub struct NativeTransform {
    process_fn: Box<dyn Fn(RuntimeEnvelope) -> Result<RuntimeEnvelope, ProcessError> + Send>,
}
```

**3-layer baseline stack (Session 5 D13):**
| Layer | Config | Measures |
|---|---|---|
| Layer 0: single-flow | No channels, inline calls | Absolute performance floor |
| Layer 1: native-with-channels | `type = "native-transform"` | Task-per-node architecture overhead |
| Layer 2: WAFER | `type = "transform"` | Full Wasm isolation cost |

**Thesis figure:** Stacked bar chart showing where each microsecond goes.

---

## Cross-Cutting Concerns

### Observability (always-on)

| Concern | Implementation | Location |
|---|---|---|
| Per-node metrics | `NodeMetrics` (AtomicU64: processed, failed, process_ns, retries, dlq, swaps) | `wafer-core/src/metrics.rs` |
| Per-hop timing | `Instant::now()` pair in every runner loop (unconditional) | `wafer-core/src/runner/*.rs` |
| Structured logging | `tracing` spans with node_id context | All runner loops |
| Pipeline events | `broadcast::channel<PipelineEvent>` | `orchestrator/mod.rs` |
| Prometheus scrape | Feature-gated HTTP endpoint reading from NodeMetrics atomics | `api/metrics.rs` |

### Error Flow

```
Plugin returns ProcessError variant
  │
  ├─ BadInput ──────────→ DLQ immediately
  ├─ DependencyFailed ──→ RetryBuffer (exponential backoff) → DLQ on exhaustion
  ├─ ProcessingFailed ──→ RetryBuffer (N retries) → DLQ on exhaustion
  ├─ TimedOut ──────────→ Skip + log (epoch interrupt fired)
  └─ Unrecoverable ────→ Teardown → Recovering (re-instantiate from InstancePre) → Running
                          └─ If re-instantiation fails → Error (terminal)
```

### Hot-Swap Flow

```
API request: POST /nodes/:id/swap { plugin: "new.wasm" }
  │
  ├─ PREPARE: load → compile → cache → InstancePre → instantiate → validate → SwapPayload
  ├─ SIGNAL: watch_tx.send(Some(payload))
  ├─ FLIP: node loop (between messages) → flush retry buffer → replace store/bindings → init()
  └─ RETIRE: old Store dropped (RAII)
```

### Graceful Shutdown Sequence

```
CancellationToken fires
  ├─ 1. Sources: select! sees cancel → stop polling
  ├─ 2. Processing nodes: select! sees cancel → stop recv
  │     Each node on exit: ProcessingGuard drops, retry buffer flushes → DLQ
  ├─ 3. Sinks: drain remaining channel messages, call flush()
  ├─ 4. DLQ task: drain queue, flush sink, exit (5s timeout)
  ├─ 5. Epoch ticker thread: detects shutdown flag, exits
  └─ 6. Orchestrator: join all handles, set state Stopped
```

---

## Full Workspace Overview

```
wafer/
├── Cargo.toml                   # Host workspace (5 crates)
├── clippy.toml
├── rustfmt.toml
├── rust-toolchain.toml
│
├── crates/
│   ├── wafer-types/             # Domain vocabulary (zero heavy deps)
│   ├── wafer-config/            # Config parsing + DAG validation
│   ├── wafer-core/              # Runtime engine
│   ├── wafer-plugin/            # Guest SDK (standalone)
│   ├── wafer-runtime/           # Binary entry point
│   ├── wafer-loadgen/           # External MQTT load generator
│   └── waferctl/                # CLI tool
│
├── plugins/                     # Separate workspace (wasm32-wasip2)
│   ├── Cargo.toml
│   ├── .cargo/config.toml
│   ├── Makefile
│   ├── wit/                     # 4-package WIT contracts (source of truth)
│   ├── pass-through/
│   ├── ... (12 Rust plugins)
│   ├── attacks/                 # 6 attack scenario plugins
│   ├── go/                      # Polyglot: TinyGo
│   └── python/                  # Polyglot: componentize-py
│
├── eval/                        # Evaluation infrastructure
│   ├── configs/                 # Experiment pipeline configs
│   ├── loadgen/                 # Load generation profiles
│   ├── scripts/                 # RPi setup, run automation
│   ├── analysis/                # UV Python: notebooks + wafer_analysis pkg
│   ├── results/                 # Raw data (.gitignored)
│   ├── Makefile                 # make e-perf-1, make all, make figures
│   └── README.md                # Reproduction guide
│
├── docs/
│   ├── decisions/               # Phase 0 decision documents (8 sessions + this)
│   ├── benchmarks/              # Historical benchmark results
│   └── adr/                     # Architecture Decision Records
│
└── wit/                         # Symlink → plugins/wit/ (for host bindgen)
```

---

## Detailed Design References (Session-by-Session)

This section maps each session's key decisions to their implementation location, ensuring nothing is lost between design and implementation.

### Session 1: WIT Contracts & Envelope Design

> **Full spec:** `docs/decisions/2025-07-05-wit-contracts-envelope-design.md` (18 decisions)

**WIT 4-package structure** (`plugins/wit/`):
```
pipeline:types@0.1.0        → pipeline-types.wit
  ├── buffer resource (size, read, read-all)
  ├── message record (id, timestamp, source, content-type, metadata, payload: borrow<buffer>)
  ├── output-message record (same fields, payload: list<u8>)
  ├── process-error variant (5 categories: bad-input, dependency-failed, processing-failed, timed-out, unrecoverable)
  └── port-id type alias (string)

pipeline:node@0.1.0         → pipeline-node.wit
  ├── lifecycle interface (validate, init, close)
  ├── transform interface (process: message → result<output-message, process-error>)
  ├── filter interface (evaluate: message → result<bool, process-error>)
  ├── transform-node world (exports lifecycle + transform, imports types + host/logging)
  ├── filter-node world (exports lifecycle + filter, imports types + host/logging)
  └── inference-node world (exports lifecycle + transform, imports types + host/logging + wasi:nn)

pipeline:routing@0.1.0      → pipeline-routing.wit
  ├── router interface (output-ports, route: message → result<list<port-id>, process-error>)
  └── router-node world (exports lifecycle + router, imports types + host/logging)

pipeline:host@0.1.0         → pipeline-host.wit
  └── logging interface (log: func(level: log-level, message: string))
```

**Host logging implementation** (`engine/state.rs`):
- `WaferState.log_buffer: Vec<LogEntry>` accumulates logs during a Wasm call
- After call returns: flush `log_buffer` → `tracing` spans with node_id context
- `log_buffer.clear()` before each call

**Buffer lifecycle per-call** (`engine/buffer.rs` + runner loops):
1. Node loop receives `RuntimeEnvelope` from channel
2. `WaferBuffer { data: envelope.payload.clone() }` — Bytes clone = refcount bump, NO copy
3. `table.push(buffer)` → `Resource<WaferBuffer>` handle (~25ns)
4. Construct WIT `message` record with `borrow<handle>`
5. Call `process()`/`evaluate()`/`route()`
6. `table.delete(handle)` (~25ns) — buffer invalidated

**node-config shape** (WIT `init` parameter): `{ id: string, config: string }` — config is JSON.

### Session 2: Host Runtime Architecture

> **Full spec:** `docs/decisions/2025-07-06-host-runtime-architecture.md` (10 decisions)

**Bytes zero-copy from MQTT** (`node/source/mqtt.rs`):
- rumqttc `Publish.payload` is already `bytes::Bytes`
- Source does `envelope.payload = publish.payload` — zero allocation
- This is the biggest allocation hotspot fix from the current `Vec<u8>` design

**RuntimeEnvelope structure** (`envelope.rs`):
```rust
pub struct RuntimeEnvelope {
    pub header: Arc<EnvelopeHeader>,  // immutable, shared via refcount
    pub payload: Bytes,               // refcounted, zero-copy from MQTT
    pub(crate) lineage: Lineage,      // mutable per-hop, small enough to copy
}
pub struct EnvelopeHeader {
    pub id: Box<str>,
    pub timestamp: u64,
    pub source: Box<str>,
    pub content_type: Box<str>,
    pub metadata: Vec<(Box<str>, Box<str>)>,
}
pub(crate) struct Lineage {
    pub parent_id: Option<Box<str>>,
    pub trace_id: Option<Box<str>>,
}
```
- Clone cost: Arc bump + Bytes bump + Lineage copy = ~10ns total

**RetryBuffer details** (`runner/error_policy.rs`):
```rust
struct RetryBuffer {
    entries: VecDeque<RetryEntry>,
    capacity: usize,  // from config, default 100
}
struct RetryEntry {
    envelope: RuntimeEnvelope,
    category: ErrorCategory,
    retry_count: u32,
    next_attempt_at: Instant,  // backoff_base * 2^retry_count, capped at 30s
}
```
- **Priority over fresh messages:** check retry buffer FIRST each loop iteration
- **Backoff:** exponential, capped at 30s max
- **Full → DLQ:** if retry buffer full, new errors go directly to DLQ (reason: `RetryBufferFull`)
- **Hot-swap flush:** ALL pending retries flushed to DLQ with reason `HotSwapDrain`
- **Shutdown flush:** ALL pending retries flushed to DLQ with reason `Shutdown`

**Store lifecycle** — persistent per node (NOT per-call):
- Created once at build time, lives for node's lifetime
- Per-call: reset fuel + reset epoch deadline + push buffer + call + delete buffer
- Cancel-safety: only `recv()` inside `select!`; Wasm call OUTSIDE (runs to completion, never cancelled)

### Session 3: Node Type Architecture

> **Full spec:** `docs/decisions/2025-07-06-node-type-architecture.md` (12 decisions + 3 amendments)

**WasmBindings enum** (`engine/bindings.rs`):
```rust
pub(crate) mod transform_world {
    wasmtime::component::bindgen!({ path: "wit", world: "transform-node",
        with: { "pipeline:types/types/buffer": crate::engine::WaferBuffer } });
}
pub(crate) mod filter_world {
    wasmtime::component::bindgen!({ path: "wit", world: "filter-node",
        with: { "pipeline:types/types": super::transform_world::pipeline::types::types } });
}
pub(crate) mod router_world {
    wasmtime::component::bindgen!({ path: "wit", world: "router-node",
        with: { "pipeline:types/types": super::transform_world::pipeline::types::types } });
}
pub(crate) enum WasmBindings {
    Transform(transform_world::TransformNode),
    Filter(filter_world::FilterNode),
    Router(router_world::RouterNode),
}
```

**Trait signatures** (`node/traits.rs`):
```rust
pub trait Transform: Send + 'static {
    async fn process(&mut self, input: RuntimeEnvelope) -> Result<RuntimeEnvelope>;
}
pub trait Filter: Send + 'static {
    async fn evaluate(&mut self, input: &RuntimeEnvelope) -> Result<FilterOutcome>;
}
pub enum FilterOutcome { Pass, Drop }
pub trait Router: Send + 'static {
    fn output_ports(&self) -> &[String];
    async fn route(&mut self, input: &RuntimeEnvelope) -> Result<RouteOutcome>;
}
pub enum RouteOutcome { Ports(Vec<String>), Drop }
```

**Merge = multi-sender topology** (NOT a node):
- Two edges pointing to same destination → builder clones sender
- `tokio::mpsc` is inherently multi-producer, single-consumer
- Zero cost: no extra task, no extra queue hop, no Wasm

### Session 4: Config Schema

> **Full spec:** `docs/decisions/2025-07-06-config-schema-pipeline-ux.md` (14 decisions)

**Semantic validation checks** (`wafer-config/src/validation.rs`):
- No cycles in DAG
- No orphan nodes (every node referenced by at least one edge)
- Router edges MUST specify `port`
- Only Transform/Sink may have multiple inbound edges (merge)
- Filter/Router: exactly one inbound edge
- Source nodes: zero inbound edges
- Sink nodes: zero outbound edges
- DLQ must be configured if any edge uses `overflow = "dead-letter"`
- `stdin`/`stdout` singleton constraint
- All edge `from`/`to` reference existing node IDs

**Error policy cascade** (resolved at build time in `orchestrator/builder.rs`):
- Pipeline-level `[error_policy]` → defaults for ALL Wasm nodes
- Per-node `[nodes.X.error_policy]` → overrides specific categories
- Result: `ResolvedErrorPolicy` per node (merged struct)

**Plugin config serialization:** `[nodes.X.config]` TOML table → JSON string → WIT `init(node-config { id, config })`.

### Session 5: Orchestrator & Runtime Simplification

> **Full spec:** `docs/decisions/2025-07-12-orchestrator-runtime-simplification.md` (13 decisions)

**Watch channel hot-swap** (`orchestrator/hotswap.rs`):
```rust
type SwapSignal = watch::Receiver<Option<SwapPayload>>;
struct SwapPayload {
    new_store: Store<WaferState>,
    new_bindings: WasmBindings,
    new_pre: Arc<InstancePre<WaferState>>,
}
```
- Node loop checks `swap_rx.has_changed()` between messages (non-blocking)
- On change: flush retry buffer → DLQ → replace store/bindings/cached_pre → init() → resume
- "Drain time" = time to finish current message (bounded by fuel/epoch)

**Config diff types** (`wafer-config/src/diff.rs`):
```rust
enum NodeChange {
    PluginChanged { new_plugin: String },       // Full hot-swap (~9ms)
    ConfigChanged { new_config_json: String },   // Warm swap (~5µs, same InstancePre)
    BothChanged { new_plugin, new_config },      // Full hot-swap
    PolicyChanged { new_policy },                // Config update only
}
```
- Structural changes (added/removed nodes, edge changes) → require full pipeline restart

**Builder parallel compilation** (`orchestrator/builder.rs` step 4):
```rust
let mut compile_set = JoinSet::new();
for (node_id, wasm_bytes) in wasm_nodes {
    compile_set.spawn(async move {
        let component = cache.get_or_compile(&engine, &wasm_bytes)?;
        let pre = linker.instantiate_pre(&component)?;
        Ok((node_id, Arc::new(pre)))
    });
}
```
- Cold start: 5 plugins × ~30ms → ~40ms parallel (4 cores)
- Warm start (cache hit): ~3ms parallel

### Session 6: Plugins & Guest SDK

> **Full spec:** `docs/decisions/2025-07-12-plugin-rewrite-guest-sdk.md` (17 decisions)

**Plugin state pattern** (all stateful plugins via `wafer-plugin` macros):
```rust
// thread_local! + RefCell<Option<T>> — safe interior mutability in single-threaded Wasm
define_state!(MyConfig);           // declares the thread_local
set_state!(parsed_config);         // in init()
with_state!(cfg => { /* use */ }); // in process/evaluate/route
```
- State is LOST on hot-swap (new Store = fresh linear memory) — BY DESIGN
- Hot-swap recalibration (EWMA reset, new config) is the intended behavior

**Guest SDK macro rationale** (`wafer-plugin/src/lib.rs`):
- `macro_rules!` NOT functions — WIT types are generated locally per-plugin by `wit_bindgen::generate!`
- A separate crate cannot reference those types (they don't exist until generated)
- Macros expand at call site where WIT types are in scope
- Only `parse_config<T>()` is a regular function (no WIT types involved)

**Plugin build priority order:**
1. `wafer-plugin` SDK crate (foundation)
2. WIT files (4-package rewrite)
3. `wafer-pass-through` (validates build chain)
4. `PluginTestHarness` (Level 2 testing)
5. `wafer-threshold-filter` (first filter-node)
6. `wafer-content-router` (first router-node)
7. `TestPipeline` E2E infrastructure
8. `wafer-json-parse` (Pipeline A complete)
9. `wafer-anomaly-detector` (strongest narrative plugin)
10. Pipeline A E2E test
11. Remaining plugins (inference, complex, polyglot, attacks)

**Binary size thesis argument:**
| Method | Simple Filter | Complex Transform |
|---|---|---|
| Docker container | ~50-100 MB | ~100-200 MB |
| WAFER Wasm (no serde) | **~5 KB** | — |
| WAFER Wasm (with serde) | — | **~150-300 KB** |

10,000× smaller for simple plugins. 300-600× for complex ones.

### Session 7: Performance Optimizations

> **Full spec:** `docs/decisions/2025-07-12-performance-optimizations.md` (10 + 4 code quality decisions)

**foldhash usage** (internal maps with trusted keys ONLY):
```rust
type WaferHashMap<K, V> = HashMap<K, V, foldhash::fast::RandomState>;
```
- Apply to: builder node map, edge routing table, InstancePre cache, config lookup
- Do NOT apply to: user-facing types, serializable types, anything with untrusted keys

**Epoch ticker cleanup** (`engine/mod.rs`):
```rust
let shutdown = Arc::new(AtomicBool::new(false));
std::thread::Builder::new()
    .name("wafer-epoch-ticker")
    .spawn(move || {
        while !shutdown.load(Ordering::Relaxed) {
            std::thread::sleep(Duration::from_millis(epoch_tick_ms));
            engine.increment_epoch();
        }
    })?;
```

**AOT cache key format:** `{blake3_hex(wasm_bytes)}-{os}-{arch}-wt{wasmtime_major}.cwasm`
- Eviction: remove stale file on deserialization failure (wasmtime version mismatch)
- Cache directory: `{registry.cache_dir}/components/` or `~/.cache/wafer/components/`

**4 metering configurations** (thesis overhead decomposition):
| Config | fuel | epoch | Purpose |
|---|---|---|---|
| A (Production) | true | true | Default, strongest containment |
| B (Fuel only) | true | false | Isolate fuel overhead |
| C (Epoch only) | false | true | Isolate epoch overhead |
| D (Neither) | false | false | Pure Wasm boundary baseline |

### Session 8: Evaluation Harness Design

> **Full spec:** `docs/decisions/2025-07-12-evaluation-harness-design.md` (15 + 6 experiment decisions)

**BenchSource specifics** (`node/source/mod.rs`):
```rust
pub struct BenchSource {
    payloads: Vec<Vec<u8>>,
    rate_per_sec: f64,
    total_messages: u64,
    warmup_messages: u64,
    sequence: u64,         // monotonic for gap detection
    start_time: Instant,
}
```
- Constant arrival rate via `tokio::time::interval`
- Stamps each message with `intended_publish_ns` (NOT actual send time) → prevents coordinated omission
- Sequence number in metadata for SequenceTracker

**BenchSink specifics** (`node/sink/mod.rs`):
```rust
pub struct BenchSink {
    histogram: HdrHistogram,           // 3 sig digits, 1µs–10s range
    warmup_until: Instant,             // 30s exclusion
    sequence_tracker: SequenceTracker, // gap/duplicate detection (E-Swap-2)
    hotswap_recorder: HotSwapRecorder, // version boundary (E-Swap-1)
    throughput_samples: Vec<(Instant, u64)>, // periodic snapshots
    csv_writer: Option<CsvWriter>,
}
```
- `SequenceTracker`: detects gaps (lost) and duplicates → validates E-Swap-2 zero-loss
- `HotSwapRecorder`: tracks `plugin_version` metadata, detects v1→v2 boundary
- After run: exports `.hdr` + `.csv` files

**MemoryRecorder** (`metrics.rs` or dedicated file):
```rust
pub struct MemoryRecorder { samples: Vec<(Instant, u64)> }
// Reads /proc/self/statm at 1Hz (Linux), memory_stats crate (macOS dev)
```
- E-Perf-3/6: memory footprint, per-node scaling via incremental measurement

**wafer-loadgen RateController** (`crates/wafer-loadgen/`):
- Token-bucket rate control (lightbench pattern)
- Embeds `intended_publish_ns` in JSON payload
- Profiles: steady (500/1000/2000), burst (2× for 10s/60s), ramp (100→5000 over 5min)
- Hot-swap trigger profile: steady + API call at scheduled time

**Cross-architecture approach** (methodology, not code):
- Cross-compile identical binary for ARM64 (RPi 4) and x86-64
- Same `.wasm` plugins (platform-independent)
- Report overhead RATIO (Wasm/Native) — dimensionless, portable
- Target: |ARM ratio - x86 ratio| < 5 percentage points

**eKuiper comparison** (`eval/configs/ekuiper/`):
- Same MQTT broker (mosquitto), same message format, same hardware
- wafer-loadgen drives BOTH systems identically
- CPU affinity: loadgen+broker on core 0, SUT on cores 1-3
- Common subscriber computes latency from payload timestamp

**22 Experiments → Infrastructure Mapping:**

| Experiments | Source | Sink | Load Gen |
|---|---|---|---|
| E-Perf-1,2 (throughput, latency) | MqttSource | BenchSink | wafer-loadgen |
| E-Perf-3,6 (memory) | BenchSource | NullSink | in-process + MemoryRecorder |
| E-Perf-4,5,7,8,9 (per-hop, cross-arch, metering, depth, AOT) | BenchSource | BenchSink | in-process |
| E-Iso-1–6,8 (attack containment, recovery) | MemorySource | CollectorSink | TestPipeline |
| E-Iso-7 (fault isolation) | BenchSource | BenchSink | parallel-branch topology |
| E-Swap-1–6 (hot-swap) | BenchSource | BenchSink + HotSwapRecorder | + SwapTimeline |
| E-Backpressure | BenchSource (burst) | BenchSink | queue depth time-series |
| E-Density-1 (binary size) | — | — | file size measurement |
| eKuiper comparison | MqttSource | External subscriber | wafer-loadgen |

---

## Sources

- Session 1–8 decision documents (architecture constraints)
- Torvyn workspace structure (6-crate pattern, validated at similar scale)
- Rust API Guidelines: crate organization (https://rust-lang.github.io/api-guidelines/)
- Microsoft Pragmatic Rust Guidelines: module structure
- `rust-best-practices` skill: module organization patterns
