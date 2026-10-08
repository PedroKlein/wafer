# Rust in WAFER's context

> **Documentation type:** Explanation
>
> **Prerequisites:** [Learning guide orientation](README.md) and [workspace and ownership map](workspace-map.md). Basic familiarity with variables, functions, structs, and pattern matching helps.

This page explains only the Rust constructs that appear in the cited WAFER paths. It is a reading aid, not a general Rust course. The async runtime is covered in [Tokio in WAFER's context](tokio-in-context.md), and the Wasm engine in [Wasmtime in WAFER's context](wasmtime-in-context.md).

## Crates, modules, and re-exports

The root `Cargo.toml` defines one workspace with seven member packages. A package may expose a library crate, a binary crate, or both. Inside a crate, `mod` declarations divide implementation into modules. A `pub use` can then present selected names at the crate root.

`wafer-config` is the smallest example. Its `lib.rs` keeps `loader` and `validation` private but re-exports `load_config` and `validate`. A caller can write `use wafer_config::{load_config, validate}` without knowing the internal module layout. This creates a stable, narrow entry point while the implementation remains split by responsibility.

Some modules compile only when a condition holds. `crates/wafer-core/src/lib.rs` declares `pub mod api` under `#[cfg(feature = "http-api")]` and `pub mod testing` under `#[cfg(any(test, feature = "test-support"))]`. Features are declared in each crate's `[features]` table. `wafer-runtime` turns `http-api` on by default, which is how the `wafer` binary gets its control plane.

## Results and enums

`load_config` returns `Result<Config, ConfigError>`. The two type parameters mean success carries a `Config`, while failure carries a structured `ConfigError`. The `?` operator in the function returns an I/O or parse error to the caller instead of choosing a process-level response inside the library. The next section shows how `?` converts those errors.

Enums make the alternatives explicit. `NodeDef` has `Source`, `Sink`, `Transform`, `Filter`, and `Router` variants. Its `category` method matches every variant and returns a smaller `NodeCategory`. Adding a new node category therefore makes exhaustive matches fail to compile until each owner handles it.

`validate` uses a different result shape: `Result<(), Vec<ValidationError>>`. Success carries no value beyond `()`. Failure carries every semantic error accumulated during the pass. This is different from TOML parsing, which cannot construct a `Config` after a structural error.

## Errors: `?`, thiserror, and anyhow

`wafer-types`, `wafer-config`, and `wafer-core` define typed error enums with the `thiserror` derive. The `wafer` binary collects those errors in `anyhow::Error` and turns them into an exit code.

Applied to an `Err`, `?` returns early with `Err(From::from(error))`, so the error is converted into the function's own error type on the way out. `ConfigError` in `crates/wafer-config/src/error.rs` declares `Io(#[from] std::io::Error)` and `Parse(#[from] toml::de::Error)`. The `#[from]` marker makes the derive write `impl From<std::io::Error> for ConfigError` and the same for the TOML error, and each `#[error("...")]` attribute writes the `Display` text. That is why `load_config` in `crates/wafer-config/src/loader.rs` can apply `?` to both `std::fs::read_to_string` and `toml::from_str`.

`wafer-core` follows the same pattern with `WaferError` in `crates/wafer-core/src/error.rs`. That file also defines `Result<T>` as an alias for `std::result::Result<T, WaferError>`, so a `wafer-core` signature that shows `Result` with one type parameter, such as `Result<()>`, fails with `WaferError`. The file has its own `ConfigError` too, unrelated to `wafer_config::ConfigError`; check the import when you meet the name.

`run` in `crates/wafer-runtime/src/main.rs` returns `anyhow::Result`. Any error type that implements `std::error::Error + Send + Sync + 'static` converts into `anyhow::Error` through `?`, and `.context("Failed to load configuration")` wraps it in a layer with a message. `run` adds a second, typed layer with `.context(ConfigInvalid)`, where `ConfigInvalid` is a marker struct defined in the same file. When startup fails, `main` passes the error to `startup_exit_code`, which looks inside it in two ways. `downcast_ref::<ConfigInvalid>()` finds the marker. `chain()` walks every cause looking for a `WaferError::Config`, which is how a source or sink `validate()` failure during launch is recognized. Either match gives exit code 2, and any other startup error gives 1. A failure after the pipeline is running takes another path: `run` returns `Ok` with exit code 3, so that bench artifacts are flushed first. `crates/wafer-runtime/tests/exit_status.rs` runs the binary and checks these codes.

## Serde attributes on the configuration types

`load_config` never walks the TOML by hand. `toml::from_str` calls the `Deserialize` impls that `#[derive(Deserialize)]` generates for the types in `crates/wafer-types/src/config/mod.rs`, and `#[serde(...)]` attributes choose the TOML shape. `examples/dag-passthrough.toml` shows most of them:

```toml
[nodes.source]
type = "source"
kind = "stdin"

[nodes.passthrough]
type = "transform"
plugin = "../plugins/pass-through/target/wasm32-wasip2/release/wafer_pass_through.wasm"
```

- `Config` has a `nodes: HashMap<String, NodeDef>` field, so each `[nodes.NAME]` table becomes one map entry keyed by its name. A `HashMap` keeps no order, so nothing downstream can rely on the order of nodes in the file. `#[serde(default)]` on each `Config` field lets a file leave sections out.
- `NodeDef` carries `#[serde(tag = "type", rename_all = "kebab-case")]`. The `type` key inside the table names the variant, and `rename_all` maps the variant `Transform` to the string `"transform"`.
- The `Source` variant holds a `SourceDef` from `crates/wafer-types/src/config/source_sink.rs`, which is tagged with `kind`. So `[nodes.source]` is read in two steps: `type` picks `NodeDef::Source`, then `kind = "stdin"` picks `SourceDef::Stdin`.
- `PluginSpec` is `#[serde(untagged)]`, so serde tries its variants in order. A bare string becomes `WasmPath`. A table such as `{ kind = "native", function = "uppercase" }` fails that variant and matches `Structured`, whose inner enum is tagged with `kind`.
- `#[serde(deny_unknown_fields)]` on `Config`, `WasmNodeDef`, and the adapter config structs turns a misspelled key into a parse error instead of a silently ignored line. The test `unknown_keys_in_nodes_and_edges_are_rejected` checks keys such as `fule = 1000`.

## Ownership, moves, borrowing, and shared storage

Rust gives each value one owner unless the type explicitly supports shared ownership. Pipeline queues use that rule as part of their design. Sending a `RuntimeEnvelope` into a Tokio channel moves the envelope to the receiver. The sender cannot keep using that same value afterward.

A borrow such as `&Config` or `&RuntimeEnvelope` grants temporary access without transferring ownership. `build_pipeline` borrows the configuration because it only needs to inspect it. `FilterNode::evaluate` borrows an envelope because a filter decides whether to forward the original message rather than constructing a replacement.

Fan-out is the point where one owner is insufficient. `RuntimeEnvelope` implements `Clone`; its immutable header uses `Arc<EnvelopeHeader>` and its payload uses `Bytes`. Cloning increments shared-storage reference counts instead of copying those buffers. The envelope's lineage remains an ordinary cloned value. The two clone tests verify pointer sharing for the header and payload.

## Traits, trait objects, and enum dispatch

A trait describes behavior shared by types. `Lifecycle` in `crates/wafer-core/src/node/traits.rs` defines `validate`, `init`, and `close`. `Source` in `crates/wafer-core/src/node/source/mod.rs` extends it with `poll`, and `Sink` in `crates/wafer-core/src/node/sink/mod.rs` extends it with `collect`. Every native adapter, such as `StdinSource` or `MqttSink`, implements `Lifecycle` and one of those two.

Sources and sinks are an open adapter set, so orchestration stores them as trait objects. `create_source` in `crates/wafer-core/src/orchestrator/launcher.rs` returns a `Box<dyn Source + Send>` inside a `Result`, and `run_source_loop` takes one. This trait object erases the concrete adapter type while retaining the trait's callable behavior. The box gives the value an owned, fixed-size handle; `dyn Source` selects methods through dynamic dispatch. The `Send` bound permits moving the adapter into a runtime task.

Processing nodes use enum dispatch instead. `TransformNode` and `FilterNode` in `crates/wafer-core/src/node/mod.rs` each have a `Wasm` and a `Native` variant, and each method is a `match` that forwards to the variant's own type. `run_transform_loop` takes a `TransformNode` and `run_filter_loop` takes a `FilterNode`. Routers have only a Wasm form, so `run_router_loop` takes the concrete `WasmRouterNode`. An enum suits this set because it is closed and some operations exist for only one variant: `try_reconfigure` returns an error for `Native`, and `as_wasm_mut` returns `None`.

`traits.rs` also defines `Transform`, `Filter`, and `Router` traits. Only the native baseline types in `crates/wafer-core/src/node/native/mod.rs` and the `WasmRouter` stub in `crates/wafer-core/src/node/router.rs` implement them. No runner calls them; the `native_vs_wasm` bench and the native baseline tests do. `NodeBundleKind` in `crates/wafer-core/src/orchestrator/builder.rs` shows the real split: its Transform and Filter variants carry the enums, Router carries `WasmRouterNode`, and Source and Sink carry boxed trait objects.

## Async trait methods as boxed futures

`Lifecycle::init` and `Source::poll` are asynchronous, but they are not declared `async fn`. They return a boxed future:

```rust
fn poll(&mut self) -> Pin<Box<dyn Future<Output = Result<Option<RuntimeEnvelope>>> + Send + '_>>;
```

The reason is the trait object above. Rust accepts `async fn` in a trait, but such a trait cannot be used as `dyn Source`. Each implementation's `async fn` returns its own anonymous future type, and a call through a vtable needs one return type for every implementation. Returning a boxed future gives them all the same type. Each part of the signature has a job:

- `Box<dyn Future<Output = T>>` moves the implementation's future to the heap behind one type.
- `Pin` promises that the future will not move again. An async block can hold references into its own state across an `.await`, and moving it would invalidate them. `Box::pin` allocates and pins in one step.
- `+ Send` lets the runner loop that awaits the future run as a Tokio task, which may resume on another worker thread.
- `+ '_` ties the future's lifetime to the `&mut self` borrow, because the async block captures `self`.

Implementations wrap their body in `Box::pin(async move { ... })`, as `StdinSource::poll` in `crates/wafer-core/src/node/source/stdin.rs` does. The cost is one heap allocation per call. The processing path avoids it: `TransformNode::process` is a plain `async fn`, because it is an inherent method on an enum and never called through `dyn`.

## Async functions and bounded channels

An async function can pause at `.await` while other Tokio tasks run. `run_source_loop` owns a source adapter and downstream senders; `run_sink_loop` owns a sink adapter and one receiver. `PipelineOrchestrator::spawn_bundles` moves each prepared bundle into its runner task.

The pipeline builder connects nodes with bounded `mpsc::channel(capacity)` queues, which hold at most `capacity` values each; [Tokio in context](tokio-in-context.md#bounded-mpsc-between-nodes) explains how the capacity is chosen and how `send_one` sends into them.

## Arc, atomics, and RAII

`Arc<T>` is an atomically reference-counted pointer for shared ownership across tasks. WAFER uses it for immutable envelope headers and for shared state or metric handles. `Arc` does not make inner data mutable by itself; mutation still needs an appropriate atomic or lock. `NodeMetrics` in `crates/wafer-core/src/node/metrics.rs` is shared as `Arc<NodeMetrics>` between a runner task and the `PipelineHandle` that the API reads. Its counters are `AtomicU64` fields updated with `fetch_add(1, Ordering::Relaxed)`. `Relaxed` is enough because each counter is read on its own and orders no other memory.

RAII ties cleanup to ownership. `PipelineHandle::try_begin_swap` in `crates/wafer-core/src/orchestrator/pipeline.rs` claims a node's swap-in-progress flag with one `compare_exchange(false, true, Ordering::Acquire, Ordering::Relaxed)`. Only the caller that wins the exchange receives a `SwapGuard`; a concurrent caller gets a `swap-in-progress` error. The guard's `Drop` stores `false` with `Release`, which the next claimant's `Acquire` synchronizes with.

The `hot_swap` handler in `crates/wafer-core/src/api/handlers.rs` binds the guard as `let _replacement_guard = ...?;`. A name that starts with an underscore keeps the value alive to the end of the scope, while `let _ = ...` would drop the guard and release the flag at once. Every later early return through `?` drops the guard, so a failed swap cannot leave the flag stuck.

## Syntax and lints you will see often

The workspace uses edition 2024 with the toolchain pinned in `rust-toolchain.toml`.

- A let-else binds a pattern or leaves the block. `send_one` in `crates/wafer-core/src/runner/mod.rs` writes `let Ok(permit) = sender.sender.reserve().await else { ... return; };`, so a closed channel is handled in the `else` branch and the normal path stays unindented.
- A let chain joins `if let` tests with `&&`. `PipelineHandle::record_hotswap_phase` in `crates/wafer-core/src/orchestrator/pipeline.rs` writes `if let Ok(guard) = ...read() && let Some(h) = guard.get(&key)`. Let chains require edition 2024.
- The root `Cargo.toml` has a strict `[workspace.lints]` table, and every crate opts in with `[lints] workspace = true`. Clippy denies `unwrap_used`, `expect_used`, `panic`, `indexing_slicing`, and `arithmetic_side_effects`; `clippy.toml` relaxes the first four in test code. This is why production code writes `saturating_add` and `saturating_sub` instead of `+` and `-`, and `ok_or_else` or let-else instead of `unwrap`.
- A lint that must be broken is silenced with `#[expect(lint, reason = "...")]`, because the `allow_attributes` lint warns on `#[allow]`. Unlike `allow`, `expect` warns when the lint stops firing, so stale exceptions show up. `crates/wafer-core/src/lib.rs` opens with several.

## Working vocabulary

- **Arc**: `std::sync::Arc<T>`, an atomically reference-counted owner used when tasks or cloned envelopes share one allocation.
- **Async function**: A function declared with `async` whose future can suspend at `.await`.
- **Borrow**: Temporary access through a reference such as `&Config` without taking ownership.
- **Bounded channel**: A queue with a fixed capacity, such as Tokio `mpsc::channel(capacity)`.
- **Boxed future**: The return type `Pin<Box<dyn Future<Output = T> + Send + '_>>`, which lets a trait method be asynchronous and still be called through a trait object.
- **Cargo feature**: A named compile-time option from a crate's `[features]` table; code under `#[cfg(feature = "http-api")]` is compiled only when that feature is on.
- **Crate**: A Rust compilation unit. In this workspace a crate may be a library or binary target.
- **Enum**: A closed set of variants, such as `NodeDef`, that callers handle with pattern matching.
- **Enum dispatch**: Choosing behavior with a `match` over a closed enum, such as `TransformNode::Wasm` and `TransformNode::Native`, instead of through a trait object.
- **Error context**: A layer that `anyhow`'s `.context(...)` wraps around an error; `downcast_ref` and `chain` can find it again later.
- **From conversion**: An `impl From<A> for B`. `?` calls it to turn the error it receives into the function's error type, and `thiserror`'s `#[from]` generates it.
- **Internally tagged enum**: A serde enum marked `#[serde(tag = "...")]`, whose variant is named by a key inside the same table, such as `type = "transform"`.
- **Let-else**: `let PATTERN = value else { ... };`, which binds when the pattern matches and otherwise must leave the enclosing block, usually with `return`.
- **Lint expectation**: An `#[expect(lint, reason = "...")]` attribute, which silences a lint and warns if the lint no longer fires.
- **Move**: Transfer of a value's ownership, such as sending an envelope or moving a node bundle into a task.
- **Ownership**: Rust's rule that a value has an owner responsible for its lifetime and cleanup.
- **Public re-export**: A `pub use` that exposes an item through a crate or module boundary.
- **RAII**: Resource acquisition is initialization; an owned guard restores state in its `Drop` implementation.
- **Result**: `Result<T, E>`, an enum carrying either a success value or a typed error.
- **Trait object**: A value such as `Box<dyn Source + Send>` that stores an unknown concrete type behind a trait interface.
- **Untagged enum**: A serde enum marked `#[serde(untagged)]`, which tries each variant in order until one matches the input, such as `PluginSpec`.
- **Workspace**: A group of Cargo packages that share resolution, dependency declarations, and configuration.

## Status boundaries

**Current implementation:** The cited files use all concepts above. Tests verify configuration loading, accumulated validation failures, rejection of unknown TOML keys, shared envelope storage, runner message flow, the single-winner swap claim and its RAII release, and the runtime's exit codes.

**Intended design:** These Rust mechanisms reinforce ownership boundaries: data types do not perform I/O, messages move through bounded queues, adapters are owned by their tasks, and guards pair state changes with cleanup.

**Known drift:** The `Transform`, `Filter`, and `Router` traits and the `AnyNode` enum in `crates/wafer-core/src/node/mod.rs` are still public, but the runtime path does not use them; the doc comment on `AnyNode` still calls it node storage for DAG orchestration. The phrase "zero-copy envelope" remains deliberately bounded to shared host storage and borrow-only inspection; Component Model lifting/lowering still materializes strings, metadata, and Transform output bytes. Fan-in remains unordered.

## Checkpoint

Open `crates/wafer-core/src/node/source/stdin.rs` and explain why `poll` returns a boxed future instead of being an `async fn`. Then name the exit code `wafer` returns when a node table contains a misspelled key, and trace each conversion that carries the error from `toml::from_str` to `startup_exit_code`.
