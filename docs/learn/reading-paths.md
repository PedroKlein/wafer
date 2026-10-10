# Reading paths

> **Documentation type:** How-to guide
>
> **Prerequisites:** [Learning guide orientation](README.md) and the ability to navigate Rust modules.

Use one route at a time. Each route starts with the [workspace and ownership map](workspace-map.md), then follows files in the order that values cross crate boundaries. When a path reaches an unfamiliar Rust construct, use the linked entry in [Rust in WAFER's context](rust-in-context.md). For channels, cancellation tokens, and task sets use [Tokio in context](tokio-in-context.md); for the Engine, Store, Linker, and generated bindings use [Wasmtime in context](wasmtime-in-context.md). The primary vertical route is split into [Configuration to running pipeline](config-to-running-pipeline.md), [Follow one message through Wasm](message-through-wasm.md), [Cross the plugin boundary](plugin-boundary.md), [Shutdown and failure behavior](shutdown-and-failure.md), [Trace a stateless hot-swap](stateless-hot-swap.md), and [From experiment definition to evidence](evaluation-harness.md).

## Runtime maintainer route

Use this route to change or debug the running system.

1. Read the root `Cargo.toml` and `crates/wafer-types/src/config/mod.rs` to identify workspace members and the typed `Config` model.
2. Read `crates/wafer-config/src/loader.rs::load_config`, then `validation.rs::validate`. This separates deserialization from semantic validation; `validate` also runs the cycle check, `check_no_cycles`.
3. Read `crates/wafer-runtime/src/main.rs::run` (called from `main`, which builds the Tokio runtime) until `launch_pipeline_timed`. This shows the process boundary that composes configuration and execution.
4. Read `crates/wafer-core/src/orchestrator/launcher.rs::launch_pipeline_timed` for engine creation, the source and sink factories, and per-node plugin loading.
5. Read `crates/wafer-core/src/orchestrator/builder.rs::build_pipeline_with_io`, `wire_queues`, and `crates/wafer-core/src/orchestrator/pipeline.rs::spawn_bundles` to see ownership move into tasks, with [Tokio in context](tokio-in-context.md) for the primitives. The builder's `crates/wafer-core/src/dag/graph.rs::DagGraph` is a check whose result is discarded; tasks are spawned in `HashMap` order, not topological order.
6. Read `crates/wafer-core/src/queue/envelope.rs::RuntimeEnvelope` and the source, transform, and sink loops under `crates/wafer-core/src/runner/` to follow one message.
7. Read [Wasmtime in context](wasmtime-in-context.md) and [Cross the plugin boundary](plugin-boundary.md) before changing Component Model calls.
8. Read [Shutdown and failure behavior](shutdown-and-failure.md), then [Trace a stateless hot-swap](stateless-hot-swap.md), only after the ordinary message path is clear. Hot-swap replaces a guest instance between messages and does not retain mutable guest state.
9. Read [From experiment definition to evidence](evaluation-harness.md) before interpreting an evaluation artifact or evidence status.

Rust checkpoints: [Result](rust-in-context.md#result), [ownership](rust-in-context.md#ownership), [move](rust-in-context.md#move), [borrow](rust-in-context.md#borrow), [bounded channel](rust-in-context.md#bounded-channel), and [async function](rust-in-context.md#async-function).

## Plugin author route

Use this route to implement a guest component without learning the full host orchestrator.

1. Read [`docs/interfaces/wit-contracts.md`](../interfaces/wit-contracts.md) for the WIT packages and worlds.
2. Choose the world for the plugin category in `wit/worlds.wit`, then inspect one matching first-party plugin under `plugins/`.
3. Read [`docs/interfaces/plugin-sdk.md`](../interfaces/plugin-sdk.md), followed by `crates/wafer-plugin/src/lib.rs`, for the exact guest helpers.
4. Read `crates/wafer-core/src/node/wasm.rs::build_wit_message` and [Wasmtime in context](wasmtime-in-context.md) only when you need to understand host marshalling or resource cleanup.
5. Test a Transform component with `PluginTestHarness` (`crates/wafer-core/src/testing/harness.rs`, behind the `test-support` feature), which loads the built `.wasm` and calls `process` without channels or an orchestrator. `crates/wafer-core/tests/wasi_async_runner.rs` holds narrower regression tests: WASI async host calls must not panic on a Tokio worker, and active calls must finish before shutdown.

Do not copy WIT signatures or macro tables from this learning guide. The interface pages are the maintained lookup surface.

Rust checkpoints: [enum](rust-in-context.md#enum), [borrow](rust-in-context.md#borrow), [Result](rust-in-context.md#result), and [RAII](rust-in-context.md#raii).

## Architecture reader route

Use this route to connect the claimed architecture to implementation evidence without reading every adapter.

1. Read the [workspace and ownership map](workspace-map.md) to establish the implementation boundary.
2. Inspect `crates/wafer-config/src/validation.rs::check_no_cycles` and `crates/wafer-core/src/dag/graph.rs::DagGraph::from_config` for the acyclicity checks. Neither orders execution: the core graph is discarded after the check, and tasks are spawned in `HashMap` order. `crates/wafer-config/src/dag.rs` has no caller outside its tests.
3. Inspect `crates/wafer-core/src/orchestrator/builder.rs::wire_queues` and `pipeline.rs::spawn_bundles` for bounded communication and task ownership; [Configuration to running pipeline](config-to-running-pipeline.md) walks this path.
4. Inspect `crates/wafer-core/src/queue/envelope.rs::RuntimeEnvelope` and its clone tests for the message representation.
5. Inspect `wit/worlds.wit`, `crates/wafer-core/src/engine/bindings.rs`, and `node/wasm.rs` for the typed guest boundary, with [Wasmtime in context](wasmtime-in-context.md) and [Cross the plugin boundary](plugin-boundary.md).
6. Inspect `crates/wafer-core/src/runner/transform.rs` and `orchestrator/hotswap.rs` for stateless replacement, with [Trace a stateless hot-swap](stateless-hot-swap.md).
7. Read `eval/RESULT-CONTRACT.md` and the tests named by the evaluation documentation before interpreting result files.

Rust checkpoints: [crate](rust-in-context.md#crate), [Arc](rust-in-context.md#arc), [trait object](rust-in-context.md#trait-object), and [RAII](rust-in-context.md#raii).

## Status boundaries

**Current implementation:** The runtime imports configuration through `wafer-config`, uses bounded Tokio channels in the pipeline builder, and owns node tasks through `PipelineOrchestrator`.

**Intended design:** The routes follow vertical flows so that a reader sees a value enter, cross a boundary, and reach its consumer before studying adjacent subsystems.

**Known drift:** Architecture and status prose may summarize old or intended behavior. For every implementation claim, prefer the source and tests on `main`. Under a plain `cargo test`, many Wasm integration tests report `ok` without checking anything when their plugin is not built; [Build, run, and test](workspace-map.md#build-run-and-test) explains `artifact_available` and why `mise run test` does not have this gap.
