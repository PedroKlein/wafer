# RFC-010: I/O Integration & First End-to-End Pipeline

- **Status:** Implemented
- **Original session date:** 2026-07-15
- **Amends:** —
- **Amended by:** —

## Abstract

This RFC defines how native I/O sources and sinks are wired into the new orchestrator to achieve WAFER's first end-to-end pipeline execution. It introduces source and sink adapter loops (spawned as per-node tokio tasks), extends the builder to construct I/O trait objects and compile Wasm nodes during the build phase, adds a `PluginTestHarness` for direct component testing without pipeline overhead, specifies the plugin rewrite from the legacy WIT contract to the current four-package model, establishes an E2E integration test with channel-based in-memory I/O, and eliminates an unnecessary allocation in the MQTT source path. The result is a runnable pipeline: TOML config → build → spawn → messages through real Wasm → arrive at sink.

## Context

Phase 3 delivered the structural runtime: per-type runner loops, error policy executor, Wasm node wrappers, builder with receiver-keyed queue wiring, and `NewPipelineOrchestrator` with JoinSet supervision (275 tests pass, 8 commits on main). However, the pipeline had never run end-to-end — the orchestrator's `spawn_bundles()` spawned placeholder tasks that immediately awaited cancellation. Sources and sinks existed as trait implementations but were not wired into the new builder/orchestrator flow.

This phase was "first light": config → build → send N messages through real Wasm → verify all arrive at sink. The design had to comply with Session 5 D11 (graceful shutdown ordering: sources stop first, sinks drain last) and D12 (orchestrator retains only handles after spawn).

## Decisions

### Decision 1: Source Adapter Loop — poll() → Envelope → Channel

Each source runs in its own tokio task spawned from the bundle's `NodeBundleKind::Source` variant. The task owns a `Box<dyn Source>` and runs an adapter loop bridging the Source trait's `poll()` to the downstream channel. A `biased` select ensures cancel is checked before poll for prompt shutdown. `Ok(None)` means EOF — senders drop and downstream receivers see `None`, enabling finite-input pipelines to terminate naturally. Source errors are non-fatal (transient I/O) and continue polling.

### Decision 2: Sink Adapter Loop — Receive → collect() → flush on shutdown

Each sink runs in its own tokio task from `NodeBundleKind::Sink`. On graceful shutdown: the cancel fires, the loop breaks, remaining messages are drained from the channel buffer via `try_recv`, then `flush()` and `close()` are called. Batching sinks use a `select!` with a timeout arm that triggers `flush()` when the batch timer expires.

### Decision 3: Builder Sources/Sinks — Constructed During Build

The builder constructs source/sink instances from config during the build phase. The node bundle carries the constructed instance (not just metadata). Build validates with `validate()`; side effects (`init()`) happen only at spawn time inside the task.

### Decision 4: Orchestrator spawn_bundles() — Real Loops Replace Placeholders

Replace placeholder tasks in `spawn_bundles()` with real runner loop invocations for sources, sinks, and Wasm nodes.

### Decision 5: Wasm Instance in Bundle — Builder Compiles + Pre-instantiates

Extend the builder to accept a `&WaferEngine` and compile Wasm nodes during build. The bundle carries constructed `WasmTransformNode` / `WasmFilterNode` / `WasmRouterNode` instances. Build calls `validate()` for fail-fast; `init()` runs inside the spawned task.

### Decision 6: PluginTestHarness — Direct Wasm Testing Without Pipeline

A `PluginTestHarness` loads a `.wasm` component and provides direct call methods — no channels, no orchestrator, no pipeline. Handles engine creation, component loading, and Store setup to eliminate boilerplate from tests and benchmarks.

### Decision 7: pass-through Plugin Update — New WIT Contracts

The pass-through plugin was rewritten from the old `pipeline:transform@0.1.0` to the four-package model (`pipeline:types`, `pipeline:node`, `transform-node` world). Uses `borrow<buffer>` resource with `read_all()` for payload input; returns `output-message` with owned `list<u8>`.

### Decision 8: E2E Integration Test Design

An integration test proves the full pipeline path using in-memory `ChannelSource` / `ChannelSink` (no MQTT broker, no file I/O). Sends 100 messages, verifies all arrive with payload unchanged and ordering preserved.

### Decision 9: MqttSource Zero-Copy — Use `Bytes` from rumqttc

Eliminates the unnecessary `publish.payload.to_vec()` allocation. rumqttc's `Publish` already contains `bytes::Bytes` — moved directly into the envelope. Saves ~10MB/s allocation churn at 10K msg/s with 1KB payloads.

### Decision 10: Wasm Fixture Build Strategy

Feature-gated pre-built artifact approach. Integration tests require `.wasm` artifacts pre-compiled via `mise run build-plugin`. Avoids circular dependency issues from cross-compilation inside `build.rs`.

### Decision 11: File Organization

New/modified files span `runner/source.rs`, `runner/sink.rs`, `testing.rs`, `testing/channel.rs`, `orchestrator/builder.rs`, `orchestrator/pipeline.rs`, `tests/pipeline_e2e.rs`, `tests/plugin_harness.rs`, `plugins/pass-through/src/lib.rs`, and `node/source/mqtt.rs`.

## Alternatives Considered

- **Init sources/sinks inside builder (not at spawn):** Rejected because init has side effects (opens connections, MQTT subscriptions) and build should be pure validation only — matching the Wasm node pattern where `validate()` runs during build but `init()` runs after spawn.
- **Construct Wasm nodes inside spawned tasks (not in builder):** Rejected because the orchestrator couldn't fail-fast on invalid Wasm — validation failures must be caught before any task spawns.
- **build.rs for compiling test .wasm fixtures:** Rejected due to circular dependency issues — cross-compilation to `wasm32-wasip2` inside a host build script requires a separate Cargo invocation. Feature-gated pre-built approach is simpler and matches wasmtime's own test pattern.
- **Real I/O (MQTT, files) for integration tests:** Rejected because it adds external dependencies, non-determinism, and slowness. Channel-based in-memory I/O is fast, deterministic, and parallelizable.

## Related RFCs

- **RFC-005** — defines the orchestrator and builder architecture (JoinSet supervision, `spawn_bundles()`, shutdown ordering) that this RFC wires real I/O into.
- **RFC-001** — defines the WIT contracts (`pipeline:types`, `pipeline:node`, `borrow<buffer>`) that the rewritten pass-through plugin targets.
- **RFC-002** — defines `RuntimeEnvelope` with `bytes::Bytes` payload, enabling the zero-copy MQTT path (D9).
- **RFC-003** — defines the five node categories (Source, Sink, Transform, Filter, Router) and the `NodeBundleKind` enum variants extended here.
- **RFC-006** — defines the guest-side SDK patterns used in the pass-through plugin rewrite.

## Implementation Notes

- Source and sink adapter loops are implemented in `crates/wafer-core/src/runner/source.rs` and `crates/wafer-core/src/runner/sink.rs` respectively.
- `PluginTestHarness` lives in `crates/wafer-core/src/testing.rs`.
- The pass-through plugin was rewritten to the new WIT contracts and builds against the `transform-node` world.
- The builder constructs all node instances (native I/O and Wasm) during the build phase; the orchestrator's `spawn_bundles()` invokes real runner loops.
- The MQTT zero-copy fix eliminates the `to_vec()` allocation in the MQTT source.
- E2E integration tests use `ChannelSource` / `ChannelSink` and are gated behind the `integration-tests` feature flag.
- Code matches decisions; no divergence.
