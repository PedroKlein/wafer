# Reading paths

Use this page when you need one part of WAFER rather than the whole [first-read order](README.md#first-read-order). Each route lists files in the order a value crosses them. Start every route with the [workspace and ownership map](workspace-map.md), and open [Rust](rust-in-context.md), [Tokio](tokio-in-context.md), or [Wasmtime](wasmtime-in-context.md) in context when a construct is unfamiliar.

## Runtime maintainer

To change or debug the running system:

1. `crates/wafer-types/src/config/mod.rs`: the typed `Config` model.
2. `crates/wafer-config/src/loader.rs` (`load_config`), then `validation.rs` (`validate`): parsing first, semantic checks second.
3. `crates/wafer-runtime/src/main.rs`: `main` builds the Tokio runtime, and `run` calls `load_config`, `validate`, and `launch_pipeline_timed`.
4. `crates/wafer-core/src/orchestrator/launcher.rs` (`launch_pipeline_timed`): the engine, the source and sink factories, and plugin loading.
5. `crates/wafer-core/src/orchestrator/builder.rs` (`build_pipeline_with_io`, `wire_queues`) and `pipeline.rs` (`spawn_bundles`): queues, bundles, and tasks. [Configuration to running pipeline](config-to-running-pipeline.md) walks steps 2 to 5.
6. `crates/wafer-core/src/queue/envelope.rs` (`RuntimeEnvelope`) and the loops under `crates/wafer-core/src/runner/`: one message, as in [Follow one message through Wasm](message-through-wasm.md).
7. [Shutdown and failure behavior](shutdown-and-failure.md), then [Trace a stateless hot-swap](stateless-hot-swap.md), once the ordinary message path is clear.

## Plugin author

To write a guest component without learning the host orchestrator:

1. [WIT contracts](../interfaces/wit-contracts.md), then `wit/worlds.wit` to choose a world.
2. `plugins/pass-through/src/lib.rs`, the smallest complete transform, then one first-party plugin of your category under `plugins/`.
3. [Plugin SDK](../interfaces/plugin-sdk.md) and `crates/wafer-plugin/src/lib.rs` for the guest helper macros.
4. [Cross the plugin boundary](plugin-boundary.md) for what the host does with your component.
5. `PluginTestHarness` in `crates/wafer-core/src/testing/harness.rs` (behind the `test-support` feature) to call a built Transform's `process` without channels or an orchestrator.

## Architecture reader

To connect the architecture claims to code without reading every adapter:

1. [Workspace and ownership map](workspace-map.md) for the crate boundaries.
2. `crates/wafer-config/src/validation.rs` (`check_no_cycles`) and `crates/wafer-core/src/dag/graph.rs` (`DagGraph::from_config`) for the acyclicity checks.
3. `crates/wafer-core/src/orchestrator/builder.rs` (`wire_queues`) and `pipeline.rs` (`spawn_bundles`) for bounded queues and task ownership.
4. `crates/wafer-core/src/queue/envelope.rs` for the message representation and its clone tests.
5. `wit/worlds.wit`, `crates/wafer-core/src/engine/bindings.rs`, and `crates/wafer-core/src/node/wasm.rs` for the typed guest boundary, with [Cross the plugin boundary](plugin-boundary.md).
6. `crates/wafer-core/src/runner/transform.rs` and `orchestrator/hotswap.rs` for stateless replacement, with [Trace a stateless hot-swap](stateless-hot-swap.md).
7. [From experiment definition to evidence](evaluation-harness.md) and `eval/RESULT-CONTRACT.md` before reading any result file.
