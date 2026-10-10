# Cross the plugin boundary

> **Documentation type:** Tutorial
>
> **Prerequisites:** [Learning guide orientation](README.md), [Follow one message through Wasm](message-through-wasm.md), [Wasmtime in context](wasmtime-in-context.md), and familiarity with Rust traits and `Result`.

## Purpose

Trace one processing plugin from its WIT world through generated guest and host bindings into a live Wasmtime instance. The goal is to identify which side owns each value and where the runtime enforces the boundary without copying the WIT reference tables.

## Prerequisites

Keep the [WIT contracts](../interfaces/wit-contracts.md) open for signatures and the [plugin SDK reference](../interfaces/plugin-sdk.md) open for macro details. This walkthrough follows the source on `main`, which includes the asynchronous P2 host path and bounded outbound HTTP implementation.

## Flow

```mermaid
sequenceDiagram
    participant WIT as WIT worlds
    participant Guest as Guest component
    participant Registry as Plugin registry
    participant Engine as WaferEngine
    participant Node as Wasm node wrapper
    WIT->>Guest: wit_bindgen::generate!
    WIT->>Engine: wasmtime::component::bindgen!
    Registry->>Engine: resolved component bytes
    Engine->>Engine: compile and pre-instantiate
    Engine->>Node: Store plus typed bindings
    Node->>Guest: validate, init, process
    Guest-->>Node: typed result or process-error
```

1. `wit/worlds.wit` exposes four guest worlds: `transform-node`, `filter-node`, `router-node`, and `inference-node`. Their shared lifecycle and message types come from the other project WIT files. Transform, Filter, and Router remain the processing roles; `inference-node` is a Transform specialization with pinned wasi-nn imports, not another topology role. Sources and sinks remain native adapters created by `create_source` and `create_sink` in `crates/wafer-core/src/orchestrator/launcher.rs`.
2. A guest chooses one world with `wit_bindgen::generate!`, implements the generated lifecycle and processing traits, and calls `export!`. `plugins/pass-through/src/lib.rs` is the smallest complete transform example. The threshold filter reads its payload through `wafer_plugin::payload_as_str!`, the content router calls `read_all()` directly, and `plugins/mnist-inference/src/lib.rs` invokes wasi-nn through generated `inference-node` bindings.
3. The host runs `wasmtime::component::bindgen!` for each world in `crates/wafer-core/src/engine/bindings.rs`. Ordinary and inference Transform bindings reuse the canonical types, lifecycle, transform, and logging definitions while retaining distinct private pre-instantiation types.
4. `launch_pipeline_timed` builds native sources and sinks separately. For a Wasm processing node, `resolve_and_load_component` (`crates/wafer-core/src/orchestrator/launcher.rs`) asks `WaferRegistry::resolve` (`crates/wafer-core/src/registry/client.rs`) for a local or OCI artifact. `resolve` reads the bytes and computes their SHA-256, the plugin hash used for provenance and the hot-swap guard. The launcher then calls `WaferEngine::compile_cached`, which compiles through the blake3-keyed in-memory `ComponentCache` (`crates/wafer-core/src/engine/cache.rs`), so a node with identical bytes, or a later swap back to them, skips compilation.
5. `WaferEngine` (`crates/wafer-core/src/engine/loader.rs`) owns the Wasmtime `Engine`. Each `pre_instantiate_*` call builds a fresh `Linker` with the private `build_linker`, which adds asynchronous P2 WASI and HTTP interfaces. Ordinary `pre_instantiate_*` paths add their generated world; `pre_instantiate_inference` additionally registers wasi-nn before creating the typed pre-instance. Linking `wasi:http` does not grant a destination.
6. The launcher's `load_transform_node`, `load_filter_node`, or `load_router_node` creates a fresh `Store<WaferState>`, installs the memory limiter (`store.limiter`) and optional fuel or epoch limits, awaits typed instantiation, and calls `validate_and_init`. Only a Wasm Transform with `allow_inference = true` receives an ONNX-backed inference state. Wasm processing nodes receive outbound HTTP authority only for configured exact destinations. The node wrapper then owns the Store, bindings, and cached pre-instance.
7. For each call, `WasmTransformNode::process` and its filter and router counterparts in `crates/wafer-core/src/node/wasm.rs` use `build_wit_message` to install a host buffer resource and pass a borrowed handle. The runner awaits the generated host call outside cancellation `select!`. After it returns, the wrapper flushes guest logs, calls `delete_buffer`, and maps the typed result or trap into the host error model.

## Rust

The two binding generators face opposite directions. `wit_bindgen::generate!` creates traits and exports implemented by guest Rust. `wasmtime::component::bindgen!` creates typed host calls and linker helpers. [Wasmtime in context](wasmtime-in-context.md) explains the `Engine`, `Component`, `Linker`, typed pre-instance, and `Store`, and which of them nodes share. For this walk, one rule is enough: a `Store<WaferState>`, which holds the guest's memory, resource table, limits, and buffered logs, belongs to exactly one node.

The WIT input is `borrow<buffer>`, so the host retains payload ownership while the guest may request bytes. A transform returns an owned `list<u8>`; filter and router return decisions. These are different copy boundaries, not a claim that every plugin avoids payload reads.

## Design

### How the host picks a world

The node's TOML `type` and its `allow_inference` grant, never the component itself, decide which world the host links against; [Wasmtime in context](wasmtime-in-context.md#components-and-wit-worlds) traces that choice from `launch_pipeline_timed` to the matching `pre_instantiate_*`.

### Responsibilities

The WIT surface contains processing semantics, not transport protocols. Native source and sink traits own external I/O. The current launcher also has native baseline dispatch for Transform and Filter, while Router loading is Wasm-only. That baseline support does not create source-node or sink-node WIT worlds.

The Engine is shared, but each node gets a Store and instance. This keeps mutable guest memory local to one node task. The host awaits one guest call at a time. Capabilities, memory limits, fuel, epoch interruption, resource cleanup, outbound HTTP policy, and guest log forwarding remain host responsibilities; [Wasmtime in context](wasmtime-in-context.md#per-call-budgets) explains what each limit covers and what it leaves out.

Do not duplicate signatures here. When the WIT and this walkthrough disagree, the WIT and generated-call sites are authoritative.

## Status boundaries

**Current implementation:** The four WIT worlds, both binding-generation directions, registry resolution, async P2 Engine/Linker/Store lifecycle, guest lifecycle calls, buffer cleanup, and native source/sink factories are present in the cited source. Inference is default deny and restricted to Wasm Transform nodes. Outbound HTTP is default deny and restricted to exact destinations for Wasm processing nodes. Tests that fail instead of skipping exercise both capability families and store replacement on recovery and reconfigure (unit tests in `crates/wafer-core/src/node/wasm.rs`, such as `inference_recovery_and_reconfigure_keep_real_model_live` and `inference_and_outbound_http_survive_recovery_together`), the inference grant during hot-swap preparation (`crates/wafer-core/tests/inference_lifecycle.rs`), and the known-digit CPU path (`crates/wafer-runtime/tests/mnist_inference.rs`). The real-component tests in `crates/wafer-core/tests/wasi_http_capability.rs` are `#[ignore]`; `mise run test-http-security` builds their fixture and runs them.

**Intended design:** Borrowed buffers make payload transfer demand-driven and keep protocol adapters outside the guest contract. A plugin can still call `read-all`, as all four cited first-party examples currently do.

**Known drift:** Source comments that describe filter or router plugins as never reading payloads are broader than the current threshold-filter and content-router implementations. Treat the plugin code as authoritative. A fixture-gated integration test whose plugin is not built prints `SKIP:` and still reports `ok`, so it is not execution evidence.

## Checkpoint

Starting at `plugins/pass-through/src/lib.rs`, identify the guest-generated trait, the matching host-generated binding, the launcher function that builds its Store, and the call site that deletes the borrowed buffer resource. Then explain why adding a source or sink requires a native adapter rather than a new processing world.
