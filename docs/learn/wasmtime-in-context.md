# Wasmtime in WAFER's context

> **Read first:** [Follow one message through Wasm](message-through-wasm.md).

This page explains the Wasmtime and Component Model techniques used by `crates/wafer-core/src/engine/` and `crates/wafer-core/src/node/wasm.rs`, in the order the code uses them: create the engine, compile, link, pre-instantiate, create a Store, call the guest. By the end you will know which Wasmtime objects nodes share and which each node owns, how the host picks a WIT world, what `bindgen!` generates, why the payload buffer must be deleted by hand, and what fuel, epoch, and memory limits do and do not bound. The pinned Wasmtime is 48.0.2, at the git revision in the root `Cargo.toml`.

## Objects and who owns them

| Object | WAFER type and where it is made | Owner | Shared across nodes? |
|---|---|---|---|
| Engine | `wasmtime::Engine` inside `WaferEngine`, built by `WaferEngine::build` (`crates/wafer-core/src/engine/loader.rs`) | One `Arc<WaferEngine>` per pipeline launch | Yes, by every node and every hot-swap |
| Component | `Arc<Component>` from `ComponentCache::get_or_compile` (`crates/wafer-core/src/engine/cache.rs`) | The engine's cache | Yes, when nodes load identical bytes |
| Linker | `Linker<WaferState>` from `WaferEngine::build_linker` | A local variable in one `pre_instantiate_*` call | No, dropped after linking |
| Typed pre-instance | `TransformNodePre<WaferState>` and its filter, router and inference siblings | The node wrapper's `cached_pre` field | No, one per node |
| Store | `Store<WaferState>` | The node wrapper, which the runner task owns | Never |
| Bindings | Generated `TransformNode`, `FilterNode`, `RouterNode`, `InferenceNode` | The node wrapper, next to its Store | No, one per Store |

Two different types are named `TransformNode`. The one imported at the top of `crates/wafer-core/src/node/wasm.rs` is the struct that `bindgen!` generates in `crates/wafer-core/src/engine/bindings.rs`. The one in `crates/wafer-core/src/node/mod.rs` is the runner's enum with `Wasm` and `Native` variants. `FilterNode` has the same split, so check the `use` lines before you follow either name.

## Components and WIT worlds

The functions of a core WebAssembly module take and return only numbers, so anything larger has to go through linear memory. A component wraps core modules and declares its imports and exports with richer types: strings, lists, records, variants, `result` and resources. The canonical ABI defines how those values are copied into and out of the guest's linear memory. Plugins are built for the `wasm32-wasip2` target, which emits a component, and `WaferEngine::build` sets `Config::wasm_component_model(true)` so the engine accepts components.

A WIT world names the interfaces one component imports and exports. WAFER's WIT is one package, `wafer:pipeline@0.1.0`, in the top-level `wit/` directory. `wit/worlds.wit` defines four worlds: `transform-node`, `filter-node` and `router-node` each import `types` and `logging` and export `lifecycle` plus one processing interface, and `inference-node` is `transform-node` plus four `wasi:nn` imports.

The host never inspects a component to pick its world. The node's TOML `type` fixes its `NodeBundleKind`, and `launch_pipeline_timed` (`crates/wafer-core/src/orchestrator/launcher.rs`) matches on it to call `load_transform_node_dispatch`, `load_filter_node_dispatch` or `load_router_node`. For a Wasm plugin the call reaches `load_transform_node`, `load_filter_node` or `load_router_node`, each of which uses the matching `pre_instantiate_*`. For a Wasm Transform, `capabilities.allow_inference = true` selects `pre_instantiate_inference` instead of `pre_instantiate_transform`, and `wafer_config::validate` rejects the flag on any other node. A component that lacks the chosen world's exports, or imports wasi-nn without the grant, fails at pre-instantiation and launch stops.

## The Engine

A `wasmtime::Engine` holds the global configuration and the compiler. A `Component`, a `Linker` and a `Store` only work together if they come from the same engine. `WaferEngine` (`crates/wafer-core/src/engine/loader.rs`) wraps it together with the fuel budgets, the epoch deadline and the `ComponentCache`. `WaferEngine::build` sets three `Config` flags: `wasm_component_model(true)`, `consume_fuel(meters_fuel)`, and `epoch_interruption` when an epoch deadline is configured. Async behavior comes from the `bindgen!` options and the `_async` linker and instantiation functions, not from an engine flag.

Fuel and epoch checks are instructions that Cranelift compiles into the guest's machine code, so both flags are engine-wide. `WaferEngine::for_pipeline` turns fuel metering on when any `[engine.fuel]` budget or any Wasm node's own `fuel` is set, so one node's `fuel = 7` meters every Wasm node in the pipeline. `WaferEngine::fuel_budget` then gives a node with neither its own nor a role budget `NonZeroU64::MAX`, because a Store with no fuel in a metered engine traps on its first instruction. [ADR-0013](../adr/0013-aot-cache-and-metering.md) records the defaults and the reasoning.

One engine also means one epoch counter for all Stores. `WaferEngine::ensure_epoch_ticker` starts a named OS thread, `wafer-epoch-ticker`, at most once. The thread sleeps `epoch_tick_ms` (10 ms by default), calls `Engine::increment_epoch`, and exits once the last engine clone is dropped. It is a `std::thread` and not a Tokio task because a task could not be scheduled while every Tokio worker is busy running guest code. When `epoch_deadline` is unset, the compiled code contains no epoch checks and the ticks have no effect.

## Compiling and caching components

`Component::new` runs Cranelift over the component bytes and produces machine code for this engine. It is the expensive step, so WAFER does it once per distinct binary: `WaferEngine::compile_cached` calls `ComponentCache::get_or_compile`, which keys a map of `Arc<Component>` by the blake3 hash of the bytes. The launcher compiles every plugin through this cache, and hot-swap preparation uses the same cache inside `spawn_blocking`, so swapping back to a binary the engine has already compiled is a memory hit. A disk tier of serialized `.cwasm` files exists behind `WaferEngine::with_cache_dir`, but only tests construct it; the runtime cache is memory-only.

## Linking host interfaces

A component's imports are names such as `wafer:pipeline/logging` or `wasi:http/outgoing-handler`. A `Linker<T>` maps those names to host functions that run with access to the Store's data `T`, and linking fails if an import is missing or has the wrong type.

Each `pre_instantiate_*` method in `loader.rs` builds a new linker with `build_linker`, which adds the WASI P2 interfaces and the `wasi:http` interfaces. It then adds the world's own imports with a generated function:

```rust
TransformNode::add_to_linker::<_, HasSelf<WaferState>>(&mut linker, |state: &mut WaferState| state)
```

The closure projects from the Store data to the value that implements the generated host traits; `HasSelf<WaferState>` says that value is `&mut WaferState` itself. `pre_instantiate_inference` also adds the `wasi:nn` functions.

Linking fails closed. The ordinary transform linker has no `wasi:nn` functions, so a component that imports them fails to link (test `inference_component_requires_the_inference_preparation_path` in `loader.rs`). Linking `wasi:http` grants no destination on its own: every outgoing request passes through `OutboundHttpHooks::authorize` (`crates/wafer-core/src/engine/http.rs`), which allows only the node's configured exact destinations ([ADR-0016](../adr/0016-outbound-wasi-http-capability.md)).

## Pre-instantiation

`Linker::instantiate_pre` resolves and type-checks the imports once and returns an `InstancePre<WaferState>`. The generated `TransformNodePre::new` wraps it and looks up the world's exports, so a component that lacks `wafer:pipeline/transform` fails here. Instantiating from the pre-instance with `instantiate_async(&mut store)` then creates the instance inside a Store without resolving imports again.

WAFER keeps the pre-instance, behind an `Arc`, in the node wrapper's `cached_pre` field until a hot-swap replaces it. Three paths reuse it to build a fresh instance without compiling or linking again: `recover_from_cached_pre` after a trap, `try_reconfigure` for a config-only change, and process-time rollback in `crates/wafer-core/src/runner/transform.rs`, which restores the old pre-instance with `set_cached_pre` before recovering.

## The Store and WaferState

A `Store<T>` owns everything one instance needs at run time: its linear memories, tables and globals, its remaining fuel and epoch deadline, and a value of type `T` that host functions can reach. In WAFER, `T` is `WaferState` (`crates/wafer-core/src/engine/state.rs`). It holds the WASI context (which inherits host stdio or environment only when the node's `Capabilities` grant it), the `ResourceTable` that stores payload buffers and WASI resources, the `wasi:http` context and its outbound hooks, the memory and table limits, the node's capabilities, the wasi-nn context when the inference grant is set, and the buffer that collects the guest's `log` calls during one call.

Outside test code, new Stores are made in three places: the launcher's `load_*_node` functions, `recovery_store` in `node/wasm.rs`, and `new_swap_store` in `hotswap.rs`. Each installs the limiter, sets fuel when the engine meters it, and sets an epoch deadline when one is configured. Fuel is set before instantiation because a component's start code already consumes fuel.

A Store is never shared. Every guest call needs `&mut` access to it, so one Store serves one caller at a time, and the node wrapper that owns it lives inside one runner task. WAFER treats a Store whose call trapped as unusable, so recovery builds a new Store, instantiates the cached pre-instance into it, and drops the old one.

## Generated bindings with bindgen!

`wasmtime::component::bindgen!` reads the WIT at compile time and generates Rust types and functions. It runs four times in `crates/wafer-core/src/engine/bindings.rs`, one module per world: `transform_node`, `filter_node`, `inference_node` and `router_node`. The options that matter for reading the code:

- `imports: { default: async }`: host functions are trait methods that return `impl Future + Send`. The guest still sees a blocking call.
- `exports: { default: async }`: calls into the guest, such as `call_process`, are `async` and the runner awaits them.
- `with:`: in `transform_node` it maps the WIT resource `buffer` to `crate::engine::WaferBuffer`, so handles are typed `Resource<WaferBuffer>`. The other modules reuse the `transform_node` types, so there is one `Message` type and `WaferState` implements each host trait once.

The generated code is not in the repository. For the `transform_node` module it contains `TransformNode` with `add_to_linker`, `instantiate_async` and accessors such as `wafer_pipeline_transform()`; a `Guest` struct per exported interface, with `call_process`, `call_validate`, `call_init` and `call_close`; the typed pre-instance `TransformNodePre<T>`; the WIT types such as `Message` and `ProcessError`; and the host traits `HostBuffer` and `logging::Host`. On the host, `Guest` is a handle you call; in the guest bindings made by `wit_bindgen::generate!`, `Guest` is a trait the plugin implements. To browse the host side, run `cargo doc -p wafer-core --no-deps --open` and open `wafer_core::engine::bindings::transform_node`.

The host traits are implemented on `WaferState` in the second half of `bindings.rs`. `HostBuffer::size`, `read` and `read_all` look the handle up in the resource table and call the matching `WaferBuffer` method (`crates/wafer-core/src/engine/buffer.rs`). `logging::Host::log` stores the message, and `flush_logs` in `node/wasm.rs` drains it into `tracing` after each guest call.

## Resource handles

A WIT `resource` is an object that stays on one side of the boundary and is passed by handle. WAFER's `buffer` stays on the host: the guest receives a handle and calls `size`, `read` or `read-all` through it. On the host, `Resource<WaferBuffer>` is a `u32` index into the Store's `ResourceTable`, marked as owned or borrowed. `rep()` returns the index, and `Resource::new_own(rep)` and `Resource::new_borrow(rep)` build a handle from it.

`WasmTransformNode::process` and `build_wit_message` use them in three steps:

1. `push_buffer` stores a `WaferBuffer`, which is a `Bytes` clone with no payload copy, in the table and returns an owned handle.
2. `build_wit_message` keeps only its `rep()` and puts `Resource::new_borrow(rep)` in the `Message`, because the WIT field is `payload: borrow<buffer>`.
3. After `call_process` returns, including after a trap, `process` calls `delete_buffer(Resource::new_own(rep))`. `ResourceTable::delete` expects an owned handle, which is why the handle is rebuilt with `new_own`.

The explicit delete is required. When the guest's borrow ends, Wasmtime releases it but runs no destructor: the host's `HostBuffer::drop` runs only when a guest drops an owned handle. Without the delete, the table would gain one entry per message. `evaluate` and `route` follow the same steps. See [ADR-0007](../adr/0007-buffer-resource-zero-copy.md) for the design.

## Per-call budgets

WAFER sets three limits on every new Store and resets two of them before each guest call in `process`, `evaluate`, `route` and `validate_and_init` (`crates/wafer-core/src/node/wasm.rs`).

**Fuel.** In a metered engine most instructions consume one unit, and the guest traps with `Trap::OutOfFuel` when the Store reaches zero. Each call starts with `store.set_fuel(n)`, so the budget is per call, not per instance. When the engine does not meter, WAFER skips `set_fuel`, which would return an error.

**Epoch deadline.** `store.epoch_deadline_trap()` makes an expired deadline trap with `Trap::Interrupt`. `set_epoch_deadline(n)` is relative: the deadline is `n` ticks after the engine's current epoch. The ticker keeps advancing the epoch between calls, so `process` resets the deadline before every call; otherwise, once the deadline set at start-up had passed, every later call would trap at its first epoch check. Instantiation, `validate` and `init` get a longer budget: new Stores and `validate_and_init` use `lifecycle_epoch_deadline(n)`, at least 1,000 ticks, so a tight per-message deadline cannot fail the start or recovery of a healthy guest.

**Memory.** `WaferState::new_with_memory_limit` builds `StoreLimits` with `memory_size(memory_limit)`, `table_elements(20_000)` and `trap_on_grow_failure(true)`, and `store.limiter(...)` installs it. A `memory.grow` past the limit then traps instead of returning -1 to the guest. The limit is the node's `memory_limit` or the role default: 64 MiB for Transform, 16 MiB for Filter and Router.

Fuel and epochs bound time spent executing Wasm, not time a guest waits inside a host import. [Cross-cutting concepts](../architecture/06-crosscutting-concepts.md#fuel-and-epoch-metering) states the metering policy.

## Traps and how WAFER classifies them

A generated export call returns two layers of `Result`. For `call_process` the type is `wasmtime::Result<Result<OutputMessage, ProcessError>>`. The outer `Err` means Wasmtime aborted the call (a trap). The inner `Err` is a `process-error` the guest returned on purpose, and its instance is intact. The `match` at the end of `process` turns `Ok(Ok(output))` into a new envelope, `Ok(Err(wit_err))` into one of the five guest variants of `WasmProcessError` (`crates/wafer-core/src/runner/error_policy.rs`), and `Err(trap)` into `WasmProcessError::Trapped { code, message }` through `map_trap`, which reads the trap code with `downcast_ref::<wasmtime::Trap>()`. Host failures while driving the call, such as a failed `push_buffer`, also become `Trapped`, with `code: None`.

`Trap::Interrupt` and `Trap::OutOfFuel` count as budget exhaustion: outside a hot-swap canary window, the runner applies the node's `timed_out` error-policy action and, unless that action is `teardown`, replaces the Store. Any other trap condemns the Store. A memory-limit trap carries no `Trap` code, so `trap_kind` recognizes it by its message text. [Shutdown and failure behavior](shutdown-and-failure.md) follows these branches.

## Check yourself

**1. Two Transform nodes load the same `.wasm` file. Which Wasmtime objects do they share, and which does each node get for itself?**

<details markdown="1">
<summary>Answer</summary>

They share the Engine and, through the compile cache, the `Component`. Each node gets its own Linker (dropped after linking), its own typed pre-instance, its own Store, and its own bindings.

</details>

**2. Only one Filter sets `fuel = 5000`. What happens to a Transform in the same pipeline that sets no fuel?**

<details markdown="1">
<summary>Answer</summary>

Fuel metering is an engine-wide flag, so the Transform is metered too. With no budget of its own or for its role it gets `NonZeroU64::MAX` before each call, because a Store with no fuel in a metered engine would trap on its first instruction.

</details>

**3. Why must `process` call `delete_buffer` after every guest call?**

<details markdown="1">
<summary>Answer</summary>

The guest receives only a borrowed handle, and Wasmtime runs no destructor when a borrow ends. Without the explicit delete, the Store's resource table would gain one entry per message.

</details>

## Next

[Cross the plugin boundary](plugin-boundary.md) follows one plugin from its WIT world to a live instance using these objects.
