# Wasmtime in WAFER's context

> **Documentation type:** Explanation
>
> **Prerequisites:** [Workspace and ownership map](workspace-map.md) and [Rust in WAFER's context](rust-in-context.md).

This page explains the Wasmtime and Component Model techniques used by `crates/wafer-core/src/engine/` and `crates/wafer-core/src/node/wasm.rs`, in roughly the order the code uses them: create the engine, compile, link, pre-instantiate, create a Store, call the guest. It is a reading aid for those files, not a Wasmtime course. The pinned Wasmtime is 48.0.2, at the git revision in the root `Cargo.toml`.

## Objects and who owns them

| Object | WAFER type and where it is made | Owner | Shared across nodes? |
|---|---|---|---|
| Engine | `wasmtime::Engine` inside `WaferEngine`, built by `WaferEngine::build` (`crates/wafer-core/src/engine/loader.rs`) | One `Arc<WaferEngine>` per pipeline launch (`launch_pipeline_timed` in `crates/wafer-core/src/orchestrator/launcher.rs`) | Yes, by every node and every hot-swap |
| Component | `Arc<Component>` from `ComponentCache::get_or_compile` (`crates/wafer-core/src/engine/cache.rs`) | The engine's cache | Yes, when nodes load identical bytes |
| Linker | `Linker<WaferState>` from `WaferEngine::build_linker` | A local variable in one `pre_instantiate_*` call | No, dropped after linking |
| Typed pre-instance | `TransformNodePre<WaferState>` and its filter, router and inference siblings | The node wrapper's `cached_pre` field (`crates/wafer-core/src/node/wasm.rs`) | No, one per node |
| Store | `Store<WaferState>` | The node wrapper, which the runner task owns | Never |
| Bindings | Generated `TransformNode`, `FilterNode`, `RouterNode`, `InferenceNode` | The node wrapper, next to its Store | No, one per Store |

Two different types are named `TransformNode`. The one imported at the top of `crates/wafer-core/src/node/wasm.rs` is the struct that `bindgen!` generates in `crates/wafer-core/src/engine/bindings.rs`. The one in `crates/wafer-core/src/node/mod.rs` is the runner's enum with `Wasm` and `Native` variants. `FilterNode` has the same split. Check the `use` lines before you follow either name.

## Components and WIT worlds

The functions of a core WebAssembly module take and return only low-level values such as `i32`, `i64`, `f32` and `f64`, so anything larger has to go through linear memory. A component wraps one or more core modules and declares its imports and exports with richer types: strings, lists, records, variants, `result` and resources. The canonical ABI defines how values of those types are copied into and out of the guest's linear memory. Plugins are built for the `wasm32-wasip2` target (`plugins/build-plugins.sh`), which emits a component, and `WaferEngine::build` sets `Config::wasm_component_model(true)` so the engine accepts components.

A WIT world names the interfaces one component imports and exports. WAFER's WIT is one package, `wafer:pipeline@0.1.0`, in the top-level `wit/` directory. `pipeline-types.wit` defines the `buffer` resource, `message`, `output-message` and `process-error`. `pipeline-node.wit` defines `lifecycle`, `transform` and `filter`, `pipeline-routing.wit` defines `router`, and `pipeline-host.wit` defines `logging`. `wit/deps/wasi-nn` holds the pinned wasi-nn package. `crates/wafer-core/wit` is a symlink to `../../wit`, so the host's `bindgen!` calls can say `path: "wit"` relative to the crate.

`wit/worlds.wit` defines four worlds. `transform-node`, `filter-node` and `router-node` each import `types` and `logging` and export `lifecycle` plus one processing interface. `inference-node` is `transform-node` plus four `wasi:nn` imports.

The host never inspects a component to pick its world. The node's TOML `type` fixes its `NodeBundleKind`, and `launch_pipeline_timed` (`crates/wafer-core/src/orchestrator/launcher.rs`) matches on it to call `load_transform_node_dispatch`, `load_filter_node_dispatch` or `load_router_node`. A native Transform or Filter is built there without Wasmtime. For a Wasm plugin the call reaches `load_transform_node`, `load_filter_node` or `load_router_node`, each of which uses the matching `pre_instantiate_*`. For a Wasm Transform, `capabilities.allow_inference = true` selects `pre_instantiate_inference` instead of `pre_instantiate_transform`, and `wafer_config::validate` rejects the flag on any other node. A component that lacks the chosen world's exports, or imports wasi-nn without the grant, fails in `instantiate_pre` or in the generated `...Pre::new` (see Pre-instantiation below), and launch stops.

## The Engine

A `wasmtime::Engine` holds the global configuration and the compiler. A `Component`, a `Linker` and a `Store` are each created from a `&Engine`, and they only work together if they come from the same one. Cloning an `Engine` is cheap because it is reference counted inside. `WaferEngine` (`crates/wafer-core/src/engine/loader.rs`) wraps it together with the fuel budgets, the epoch deadline and the `ComponentCache`.

`WaferEngine::build` sets three `Config` flags:

- `wasm_component_model(true)`.
- `consume_fuel(meters_fuel)`. `WaferEngine::for_pipeline` sets `meters_fuel` when any `[engine.fuel]` budget or any Wasm node's own `fuel` is set.
- `epoch_interruption(engine_config.epoch_deadline.is_some())`.

It does not call `Config::async_support`, which is deprecated and has no effect in this Wasmtime version. Async behavior comes from the `bindgen!` options and the `_async` linker and instantiation functions described below.

Fuel and epoch checks are instructions that Cranelift compiles into the guest's machine code, so both flags are engine-wide. One node's `fuel = 7` therefore turns metering on for every Wasm node in the pipeline. `WaferEngine::fuel_budget` then gives a node with neither its own nor a role budget `NonZeroU64::MAX`, because a Store with no fuel in a metered engine traps on its first instruction. [ADR-0013](../adr/0013-aot-cache-and-metering.md) records the defaults and the reasoning.

One engine also means one epoch counter for all Stores. `WaferEngine::ensure_epoch_ticker` starts a named OS thread, `wafer-epoch-ticker`, at most once (a `OnceLock` guards it). The thread sleeps `epoch_tick_ms` (10 ms by default) and calls `Engine::increment_epoch`. It holds only a weak handle from `Engine::weak` and exits once the last engine clone is dropped. It is a `std::thread` and not a Tokio task because a task could not be scheduled while every Tokio worker is busy running guest code. The launcher starts the ticker for every pipeline. When `epoch_deadline` is unset, the compiled code contains no epoch checks and the ticks have no effect.

## Compiling and caching components

`Component::new` runs Cranelift over the component bytes and produces machine code for this engine. It is the expensive step, so WAFER does it once per distinct binary. `WaferEngine::compile_cached` calls `ComponentCache::get_or_compile` (`crates/wafer-core/src/engine/cache.rs`), which keys a `HashMap<[u8; 32], Arc<Component>>` by the blake3 hash of the bytes. A hit returns a clone of the `Arc`. A miss compiles without holding the map's lock, then inserts the result. The returned `CacheOutcome` (`MemoryHit`, `DiskHit` or `Compiled`) appears in hot-swap timings.

The launcher compiles every plugin through this cache in `resolve_and_load_component`. Hot-swap preparation uses the same cache inside `tokio::task::spawn_blocking` (`compile_and_link` in `crates/wafer-core/src/orchestrator/hotswap.rs`), because compiling and linking are synchronous CPU work that would otherwise stall a Tokio worker. Swapping back to a binary the engine has already compiled is a memory hit. A disk tier of serialized `.cwasm` files exists behind `WaferEngine::with_cache_dir`, but only tests construct it. The runtime cache is memory-only.

## Linking host interfaces

A component's imports are names such as `wafer:pipeline/logging` or `wasi:http/outgoing-handler`. A `Linker<T>` maps those names to host functions that run with access to the Store's data `T`. Linking checks every import of the component against the linker and fails if one is missing or has the wrong type.

Each `pre_instantiate_*` method in `loader.rs` builds a new linker with the private `build_linker`. That function first adds the WASI P2 interfaces with `wasmtime_wasi::p2::add_to_linker_async`, which requires `WaferState: WasiView`. It then adds the `wasi:http` interfaces with `wasmtime_wasi_http::p2::add_only_http_to_linker_async`. This variant does not add WASI a second time, and it requires `WaferState: WasiHttpView`. The `pre_instantiate_*` method then adds the world's own imports with a generated function:

```rust
TransformNode::add_to_linker::<_, HasSelf<WaferState>>(&mut linker, |state: &mut WaferState| state)
```

The closure projects from the Store data to the value that implements the generated host traits. `HasSelf<WaferState>` tells the generated code that this value is `&mut WaferState` itself, which is where WAFER implements those traits. `pre_instantiate_inference` also calls `wasmtime_wasi_nn::wit::add_to_linker(&mut linker, WaferState::nn_view)`.

Linking fails closed. The ordinary transform linker has no `wasi:nn` functions, so a component that imports them fails in `instantiate_pre`. The test `inference_component_requires_the_inference_preparation_path` in `loader.rs` checks this. Linking `wasi:http` grants no destination on its own: every outgoing request passes through `OutboundHttpHooks::authorize` (`crates/wafer-core/src/engine/http.rs`), which allows only the node's configured exact destinations ([ADR-0016](../adr/0016-outbound-wasi-http-capability.md)).

## Pre-instantiation

`Linker::instantiate_pre` resolves and type-checks the imports once and returns an `InstancePre<WaferState>`. The generated `TransformNodePre::new` wraps it and looks up the world's exports, so a component that lacks `wafer:pipeline/transform` fails here. Instantiating from the pre-instance with `instantiate_async(&mut store)` then creates the instance inside a Store and runs its start code without resolving imports again.

WAFER keeps the pre-instance, behind an `Arc`, in the node wrapper's `cached_pre` field until a hot-swap replaces it. Three paths reuse it to build a fresh instance without compiling or linking again: `recover_from_cached_pre` in `crates/wafer-core/src/node/wasm.rs` after a trap, `try_reconfigure` in the same file for a config-only change, and process-time rollback in `crates/wafer-core/src/runner/transform.rs`, which first restores the old pre-instance with `set_cached_pre` and then calls `recover_from_cached_pre`. `TransformPre` is an enum because a Transform's pre-instance is either a `TransformNodePre` or an `InferenceNodePre`.

## The Store and WaferState

A `Store<T>` owns everything one instance needs at run time: its linear memories, tables and globals, its remaining fuel and epoch deadline, and a value of type `T` that host functions can reach. In WAFER, `T` is `WaferState` (`crates/wafer-core/src/engine/state.rs`), which holds:

- `ctx: WasiCtx`, built by `WasiCtxBuilder`, which inherits host stdio or environment variables only when `Capabilities` grants `inherit_stdio` or `inherit_env`;
- `table: ResourceTable`, which stores `WaferBuffer` payload handles and WASI resources;
- `http_ctx: WasiHttpCtx` and `http_hooks: OutboundHttpHooks` for `wasi:http`;
- `limits: StoreLimits`, the memory and table caps;
- `capabilities`, the node's grants, which `try_hot_swap` compares before it accepts a replacement Store;
- `nn_ctx: Option<WasiNnCtx>`, present only with the inference grant;
- `log_buffer`, which collects the guest's `log` calls during one call, and `node_id`.

The `WasiView` and `WasiHttpView` impls at the end of `state.rs` return views that borrow `ctx` or `http_ctx` together with the same `table`, and `nn_view` does the same for wasi-nn. This is how the library host code in `wasmtime-wasi`, `wasmtime-wasi-http` and `wasmtime-wasi-nn` reaches WAFER's state.

Outside test code, new Stores are made in three places: the launcher's `load_*_node` functions, `recovery_store` in `node/wasm.rs`, and `new_swap_store` in `hotswap.rs`. Each creates the Store from the shared engine with `Store::new`, installs the limiter with `store.limiter(|s| s.limits_mut())`, sets fuel when the engine meters it, and calls `epoch_deadline_trap` and `set_epoch_deadline` when an epoch deadline is configured. Fuel is set before instantiation because a component's start code already consumes fuel.

A Store is never shared. Every guest call needs mutable access to it, as in `call_process(&mut self.store, ...)`, so one Store serves one caller at a time, and the node wrapper that owns it lives inside one runner task. Wasmtime frees an instance's memory only when its Store is dropped, and WAFER treats a Store whose call trapped as unusable (`WasmProcessError::Trapped` in `crates/wafer-core/src/runner/error_policy.rs`). Recovery therefore builds a new Store, instantiates the cached pre-instance into it, and drops the old Store.

## Generated bindings with bindgen!

`wasmtime::component::bindgen!` reads the WIT at compile time and generates Rust types and functions. It runs four times in `crates/wafer-core/src/engine/bindings.rs`, one module per world: `transform_node`, `filter_node`, `inference_node` and `router_node`. Every invocation passes these options:

- `imports: { default: async }`: host functions are trait methods that return `impl Future + Send`. The guest still sees a blocking call.
- `exports: { default: async }`: calls into the guest, such as `call_process`, are `async` and the runner awaits them.
- `require_store_data_send: true`: the generated code requires `WaferState: Send`. Wasmtime documents this option as rarely needed, mainly when synchronous bindings reuse asynchronous ones through `with:`.
- `with:`: in `transform_node` it maps the WIT resource `wafer:pipeline/types.buffer` to `crate::engine::WaferBuffer`, so handles are typed `Resource<WaferBuffer>`. The other three modules point `types` and `logging` at the `transform_node` module. Router and inference also reuse `lifecycle`, and inference reuses `transform`. There is one `Message` type, and `WaferState` implements each host trait once.

No `trappable` flag is set, so WAFER's host functions return plain values such as `u64` or `Vec<u8>` and cannot raise a trap through their return type. Among WAFER's own host traits, only the resource destructor `drop` returns `wasmtime::Result<()>`.

The generated code is not in the repository. For the `transform_node` module it contains:

- `TransformNode`, with `add_to_linker`, `instantiate_async` and the accessors `wafer_pipeline_transform()` and `wafer_pipeline_lifecycle()`;
- a `Guest` struct for each exported interface: the transform one has `call_process`, the lifecycle one has `call_validate`, `call_init` and `call_close`;
- `TransformNodePre<T>`, the typed pre-instance;
- the WIT types `wafer::pipeline::types::{Message, OutputMessage, ProcessError, LogLevel}`;
- the host traits `wafer::pipeline::types::Host`, `wafer::pipeline::types::HostBuffer` and `wafer::pipeline::logging::Host`.

On the host, `Guest` is a handle you call. In the guest bindings made by `wit_bindgen::generate!` (see `plugins/pass-through/src/lib.rs`), `Guest` is a trait the plugin implements. To browse the host side, run `cargo doc -p wafer-core --no-deps --open` and open `wafer_core::engine::bindings::transform_node`. Add `--document-private-items` to see the crate-private `router_node` and `inference_node`. Setting `WASMTIME_DEBUG_BINDGEN=1` while wafer-core recompiles also writes each expansion to a file such as `transform-node0.rs` in the macro crate's build output under `target/`.

The host traits are implemented on `WaferState` in the second half of `bindings.rs`. `HostBuffer::size`, `read` and `read_all` look the handle up with `self.table().get(&resource)` and call the matching method of `WaferBuffer` (`crates/wafer-core/src/engine/buffer.rs`). The work is synchronous, so each returns `std::future::ready(value)` to satisfy the `impl Future` signature. `logging::Host::log` maps the WIT level and calls `push_log`, and `flush_logs` in `node/wasm.rs` drains the buffer into `tracing` after each guest call.

## Resource handles

A WIT `resource` is an object that stays on one side of the boundary and is passed by handle. WAFER's `buffer` stays on the host: the guest receives a handle and calls `size`, `read` or `read-all` through it. On the host, `Resource<WaferBuffer>` is a typed `u32` index into the Store's `ResourceTable` plus internal state that records whether the handle is owned or borrowed. `rep()` returns the index, and `Resource::new_own(rep)` and `Resource::new_borrow(rep)` build a handle from it.

`WasmTransformNode::process` and `build_wit_message` use them in three steps:

1. `push_buffer` stores a `WaferBuffer`, which is a `Bytes` clone with no payload copy, in the table and returns an owned handle.
2. `build_wit_message` keeps only its `rep()` and puts `Resource::new_borrow(rep)` in the `Message`, because the WIT field is `payload: borrow<buffer>`.
3. After `call_process` returns, including after a trap, `process` calls `delete_buffer(Resource::new_own(rep))`. `ResourceTable::delete` expects an owned handle, which is why the handle is rebuilt with `new_own`.

The explicit delete is required. When the guest's borrow ends, Wasmtime releases it but runs no destructor: the host's `HostBuffer::drop` runs only when a guest drops an owned handle. Without the delete, the table would gain one entry per message. `evaluate` and `route` follow the same steps. The payload is copied only if the guest reads it: `WaferBuffer::read_all` copies with `to_vec`, and Wasmtime then copies the returned `list<u8>` into guest memory. See [Follow one message through Wasm](message-through-wasm.md) and [ADR-0007](../adr/0007-buffer-resource-zero-copy.md).

## Per-call budgets

WAFER sets three limits on every new Store and resets two of them before each guest call in `process`, `evaluate`, `route` and `validate_and_init` (`crates/wafer-core/src/node/wasm.rs`).

**Fuel.** In a metered engine most instructions consume one unit, and the guest traps with `Trap::OutOfFuel` when the Store reaches zero. Each call starts with `store.set_fuel(n)`, so the budget is per call, not per instance. The node's `fuel_limit` is `None` when the engine does not meter, and WAFER then skips `set_fuel`, which would return an error.

**Epoch deadline.** `store.epoch_deadline_trap()` makes an expired deadline trap with `Trap::Interrupt`. `set_epoch_deadline(n)` is relative: the deadline is `n` ticks after the engine's current epoch. The ticker keeps advancing the epoch between calls, so `process` resets the deadline before every call. Without the reset, the deadline would stay where `validate_and_init` last set it, and once that passed every later call would trap at its first epoch check. A call can therefore run for roughly `n` ticks of `epoch_tick_ms` each.

**Memory.** `WaferState::new_with_memory_limit` builds `StoreLimits` with `memory_size(memory_limit)`, `table_elements(20_000)` and `trap_on_grow_failure(true)`. `store.limiter(|s| s.limits_mut())` installs it as the Store's `ResourceLimiter`. A `memory.grow` past the limit then traps instead of returning -1 to the guest. The limit is the node's `memory_limit` or the role default: 64 MiB for Transform, 16 MiB for Filter and Router. It covers guest linear memory and tables, not host-side WASI resources.

Fuel and epochs bound time spent executing Wasm, not time a guest waits inside a host import. [Cross-cutting concepts](../architecture/06-crosscutting-concepts.md#fuel-and-epoch-metering) states the metering policy.

## Traps and how WAFER classifies them

A generated export call returns two layers of `Result`. For `call_process` the type is `wasmtime::Result<Result<OutputMessage, ProcessError>>`. The outer `Err` means Wasmtime aborted the call. The inner `Err` is a `process-error` the guest returned on purpose, and its instance is intact. The `match` at the end of `process` handles three cases:

- `Ok(Ok(output))` becomes a new envelope through `RuntimeEnvelope::from_guest_output`.
- `Ok(Err(wit_err))` goes through `map_process_error` to one of the five guest variants of `WasmProcessError` (`crates/wafer-core/src/runner/error_policy.rs`).
- `Err(trap)` goes through `map_trap`, which returns `WasmProcessError::Trapped { code, message }`.

Wasmtime wraps a trap in context such as a backtrace, so `map_trap` reads the trap code with `err.downcast_ref::<wasmtime::Trap>()` and keeps the whole chain in `message` with `{err:#}`. Host failures while driving the call, such as a failed `push_buffer`, also become `Trapped`, with `code: None`.

`WasmProcessError::is_budget_exhausted` is true for `Trap::Interrupt` and `Trap::OutOfFuel`. Outside a hot-swap canary window, the runner's `recover_after_timeout` (one each in `transform.rs`, `filter.rs` and `router.rs` under `crates/wafer-core/src/runner/`) applies the node's `timed_out` error-policy action and, unless that action is `teardown`, replaces the Store. Any other trap condemns the Store: the runner recovers from the cached pre-instance, or rolls back during a canary window. `trap_kind` labels traps for metrics. A memory-limit trap carries no `Trap` code, so it is recognized by the message text in `MEMORY_LIMIT_MARKER`. [Shutdown and failure behavior](shutdown-and-failure.md) follows these branches.

## Working vocabulary

- **Engine**: The `wasmtime::Engine` that holds configuration and the compiler; WAFER keeps one per pipeline inside `WaferEngine`.
- **Component**: A compiled Component Model binary (`wasmtime::component::Component`), shared through `ComponentCache` as `Arc<Component>`.
- **Linker**: A `Linker<WaferState>` that maps WIT import names to host functions; WAFER builds one per `pre_instantiate_*` call.
- **InstancePre**: A component whose imports are already resolved against a linker, ready to instantiate cheaply; WAFER keeps its typed form, such as `TransformNodePre`, in each node's `cached_pre`.
- **Store**: The `Store<WaferState>` that owns one instance's memory, fuel, epoch deadline and host state; each Wasm node has its own.
- **WIT world**: A named set of WIT imports and exports that a component must match; WAFER has four in `wit/worlds.wit`.
- **Resource handle**: A `Resource<T>` index into a Store's `ResourceTable`, marked as owned or borrowed, such as the `borrow<buffer>` payload handle.
- **Fuel**: A per-Store instruction budget; WAFER resets it before every guest call and the guest traps with `Trap::OutOfFuel` when it runs out.
- **Epoch deadline**: A per-Store limit counted in engine epoch ticks; WAFER resets it before every guest call and the guest traps with `Trap::Interrupt` when it passes.

## Status boundaries

**Current implementation:** One engine per pipeline with engine-wide fuel and epoch flags, a memory-only compile cache, a new linker per pre-instantiation, one typed pre-instance and one Store per Wasm node, async bindings for four worlds, explicit buffer deletion after each call, and per-call fuel and epoch resets, all in the files cited above.

**Intended design:** The Engine and compiled components hold no guest state, so nodes share them, while everything a guest can change stays in its node's Store. [ADR-0001](../adr/0001-wasmtime-runtime.md), [ADR-0007](../adr/0007-buffer-resource-zero-copy.md), [ADR-0013](../adr/0013-aot-cache-and-metering.md) and [ADR-0016](../adr/0016-outbound-wasi-http-capability.md) give the reasons for the runtime, the buffer resource, metering and outbound HTTP.

**Known drift:** The doc comment on `HostBuffer::drop` in `crates/wafer-core/src/engine/bindings.rs` says it runs when a `borrow<buffer>` goes out of scope. Wasmtime runs the destructor only for owned handles, so the explicit `delete_buffer` is what frees each payload entry. `wasmtime_version_major` in `crates/wafer-core/src/engine/cache.rs` returns wafer-core's own major version, not Wasmtime's. This affects only the disk tier, which the runtime does not use. `engine::WasmBindings` is exported but plays no part in choosing a world.

## Checkpoint

1. Two Transform nodes load the same `.wasm` file. Which of Engine, Component, Linker, pre-instance and Store do they share, and which does each node get for itself?
2. Only one Filter sets `fuel = 5000`. What fuel budget does a Transform in the same pipeline get before each call, and why does it need one at all?
3. Why does `process` call `set_epoch_deadline` before every call, and what would a later message see without that reset once enough ticks had passed?
4. A guest calls `read-all()` on its payload. Name the host function that runs, the point where the bytes are copied, and the reason `process` must still call `delete_buffer` afterwards.
