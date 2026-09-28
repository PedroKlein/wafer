# Reading paths

> **Documentation type:** How-to guide
>
> **Prerequisites:** [Learning guide orientation](README.md) and the ability to navigate Rust modules.

Use one route at a time. Each route starts with the [workspace and ownership map](workspace-map.md), then follows files in the order that values cross crate boundaries. When a path reaches an unfamiliar Rust construct, use the linked entry in [Rust in WAFER's context](rust-in-context.md). The primary vertical route is split into [Configuration to running pipeline](config-to-running-pipeline.md), [Follow one message through Wasm](message-through-wasm.md), [Cross the plugin boundary](plugin-boundary.md), [Trace a stateless hot-swap](stateless-hot-swap.md), [Shutdown and failure behavior](shutdown-and-failure.md), and [From experiment definition to evidence](evaluation-harness.md).

## Runtime maintainer route

Use this route to change or debug the running system.

1. Read the root `Cargo.toml` and `crates/wafer-types/src/config/mod.rs` to identify workspace members and the typed `Config` model.
2. Read `crates/wafer-config/src/loader.rs::load_config`, then `validation.rs::validate`, then `dag.rs::DagGraph::from_config`. This separates deserialization, semantic validation, and structural graph construction.
3. Read `crates/wafer-runtime/src/main.rs::run` (called from `main`) until `launch_pipeline_timed`. This shows the process boundary that composes configuration and execution.
4. Read `crates/wafer-core/src/orchestrator/builder.rs::build_pipeline_with_io`, `wire_queues`, and `crates/wafer-core/src/orchestrator/pipeline.rs::spawn_bundles` to see ownership move into tasks.
5. Read `crates/wafer-core/src/queue/envelope.rs::RuntimeEnvelope` and the source, transform, and sink loops under `crates/wafer-core/src/runner/` to follow one message.
6. Read [Cross the plugin boundary](plugin-boundary.md) before changing Component Model calls.
7. Read [Trace a stateless hot-swap](stateless-hot-swap.md) and the shutdown paths only after the ordinary message path is clear. Hot-swap replaces a guest instance between messages and does not retain mutable guest state.
8. Read [From experiment definition to evidence](evaluation-harness.md) before interpreting an evaluation artifact or evidence status.

Rust checkpoints: [Result](rust-in-context.md#result), [ownership](rust-in-context.md#ownership), [move](rust-in-context.md#move), [borrow](rust-in-context.md#borrow), [bounded channel](rust-in-context.md#bounded-channel), and [async function](rust-in-context.md#async-function).

## Plugin author route

Use this route to implement a guest component without learning the full host orchestrator.

1. Read [`docs/interfaces/wit-contracts.md`](../interfaces/wit-contracts.md) for the WIT packages and worlds.
2. Choose the world for the plugin category in `wit/worlds.wit`, then inspect one matching first-party plugin under `plugins/`.
3. Read [`docs/interfaces/plugin-sdk.md`](../interfaces/plugin-sdk.md), followed by `crates/wafer-plugin/src/lib.rs`, for the exact guest helpers.
4. Read `crates/wafer-core/src/node/wasm.rs::build_wit_message` only when you need to understand host marshalling or resource cleanup.
5. Confirm behavior with the plugin's tests and the host boundary test in `crates/wafer-core/tests/wasi_async_runner.rs`.

Do not copy WIT signatures or macro tables from this learning guide. The interface pages are the maintained lookup surface.

Rust checkpoints: [enum](rust-in-context.md#enum), [borrow](rust-in-context.md#borrow), [Result](rust-in-context.md#result), and [RAII](rust-in-context.md#raii).

## Thesis reader route

Use this route to connect the claimed architecture to implementation evidence without reading every adapter.

1. Read the [workspace and ownership map](workspace-map.md) to establish the implementation boundary.
2. Inspect `crates/wafer-config/src/dag.rs::DagGraph::from_config` and `topo_order` for the declared DAG model.
3. Inspect `crates/wafer-core/src/orchestrator/builder.rs::wire_queues` and `pipeline.rs::spawn_bundles` for bounded communication and task ownership.
4. Inspect `crates/wafer-core/src/queue/envelope.rs::RuntimeEnvelope` and its clone tests for the message representation.
5. Inspect `wit/worlds.wit`, `crates/wafer-core/src/engine/bindings.rs`, and `node/wasm.rs` for the typed guest boundary.
6. Inspect `crates/wafer-core/src/runner/transform.rs` and `orchestrator/hotswap.rs` for stateless replacement.
7. Read `eval/RESULT-CONTRACT.md` and the tests named by the evaluation documentation before interpreting result files.

Rust checkpoints: [crate](rust-in-context.md#crate), [Arc](rust-in-context.md#arc), [trait object](rust-in-context.md#trait-object), and [RAII](rust-in-context.md#raii).

## Status boundaries

**Current implementation:** The runtime imports configuration through `wafer-config`, uses bounded Tokio channels in the pipeline builder, and owns node tasks through `PipelineOrchestrator`.

**Intended design:** The routes follow vertical flows so that a reader sees a value enter, cross a boundary, and reach its consumer before studying adjacent subsystems.

**Known drift:** Architecture and status prose may summarize old or intended behavior. For every implementation claim, prefer the source and named tests at the pinned commit. Some Wasm integration tests require prebuilt component fixtures; a skipped test is conditional evidence, not a passing runtime demonstration.

## Evidence

- **Source:** [`crates/wafer-runtime/src/main.rs`](../../crates/wafer-runtime/src/main.rs) | symbols: `fn main() -> ExitCode`, `async fn run`, `launch_pipeline_timed`
- **Source:** [`crates/wafer-core/src/orchestrator/builder.rs`](../../crates/wafer-core/src/orchestrator/builder.rs) | symbols: `pub fn build_pipeline_with_io`, `fn wire_queues`
- **Test:** [`crates/wafer-core/src/orchestrator/pipeline.rs`](../../crates/wafer-core/src/orchestrator/pipeline.rs) | symbols: `async fn test_spawn_creates_tasks()`, `async fn test_source_sink_real_loops_process_messages()`
