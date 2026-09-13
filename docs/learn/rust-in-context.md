# Rust in WAFER's context

> **Documentation type:** Explanation
>
> **Prerequisites:** [Learning guide orientation](README.md) and [workspace and ownership map](workspace-map.md). Basic familiarity with variables, functions, structs, and pattern matching helps.

This page explains only the Rust constructs that appear in the cited WAFER paths. It is a reading aid, not a general Rust course.

## Crates, modules, and re-exports

The root `Cargo.toml` defines one workspace with seven member packages. A package may expose a library crate, a binary crate, or both. Inside a crate, `mod` declarations divide implementation into modules. A `pub use` can then present selected names at the crate root.

`wafer-config` is the smallest example. Its `lib.rs` keeps `loader` and `validation` private but re-exports `load_config` and `validate`. A caller can write `use wafer_config::{load_config, validate}` without knowing the internal module layout. This creates a stable, narrow entry point while the implementation remains split by responsibility.

## Results and enums

`load_config` returns `Result<Config, ConfigError>`. The two type parameters mean success carries a `Config`, while failure carries a structured `ConfigError`. The `?` operator in the function returns an I/O or parse error to the caller instead of choosing a process-level response inside the library.

Enums make the alternatives explicit. `NodeDef` has `Source`, `Sink`, `Transform`, `Filter`, and `Router` variants. Its `category` method matches every variant and returns a smaller `NodeCategory`. Adding a new node category therefore makes exhaustive matches fail to compile until each owner handles it.

`validate` uses a different result shape: `Result<(), Vec<ValidationError>>`. Success carries no value beyond `()`. Failure carries every semantic error accumulated during the pass. This is different from TOML parsing, which cannot construct a `Config` after a structural error.

## Ownership, moves, borrowing, and shared storage

Rust gives each value one owner unless the type explicitly supports shared ownership. Pipeline queues use that rule as part of their design. Sending a `RuntimeEnvelope` into a Tokio channel moves the envelope to the receiver. The sender cannot keep using that same value afterward.

A borrow such as `&Config` or `&RuntimeEnvelope` grants temporary access without transferring ownership. `build_pipeline` borrows the configuration because it only needs to inspect it. The filter trait borrows an envelope because a filter decides whether to forward the original message rather than constructing a replacement.

Fan-out is the point where one owner is insufficient. `RuntimeEnvelope` implements `Clone`; its immutable header uses `Arc<EnvelopeHeader>` and its payload uses `Bytes`. Cloning increments shared-storage reference counts instead of copying those buffers. The envelope's lineage remains an ordinary cloned value. The two clone tests verify pointer sharing for the header and payload.

## Traits and trait objects

A trait describes behavior shared by types. `Lifecycle` defines `validate`, `init`, and `close`; `Transform`, `Filter`, and `Router` add category-specific processing methods.

Native sources and sinks are open adapter sets, so orchestration stores values such as `Box<dyn Source + Send>`. This trait object erases the concrete adapter type while retaining the trait's callable behavior. The box gives the value an owned, fixed-size handle; `dyn Source` selects methods through dynamic dispatch. The `Send` bound permits moving the adapter into a runtime task.

Configuration node categories use an enum instead. That set is closed and every variant must be handled. The distinction is visible in `NodeBundleKind`: its outer categories are enum variants, while source and sink variants carry boxed trait objects.

## Async functions and bounded channels

An async function can pause at `.await` while other Tokio tasks run. `run_source_loop` owns a source adapter and downstream senders; `run_sink_loop` owns a sink adapter and one receiver. `PipelineOrchestrator::spawn_bundles` moves each prepared bundle into its runner task.

The pipeline builder calls `mpsc::channel(capacity)`. A bounded channel accepts at most that many queued values. On the current downstream path, `send_one` waits for a permit with `sender.reserve().await`; waiting there applies backpressure. One receiver is created per destination, and upstream nodes receive cloned sender handles. That shape permits fan-in without multiple consumers racing for the same destination stream.

## Arc and RAII

`Arc<T>` is an atomically reference-counted pointer for shared ownership across tasks. WAFER uses it for immutable envelope headers and for shared state or metric handles. `Arc` does not make inner data mutable by itself; mutation still needs an appropriate atomic or lock.

RAII ties cleanup to ownership. `ProcessingGuard` marks a node as processing when constructed and clears that flag in `Drop`. Early returns through `?` still drop the guard, so callers cannot accidentally leave the state stuck. `SwapGuard` applies the same pattern to the per-node swap-in-progress flag.

## Working vocabulary

- **Arc**: `std::sync::Arc<T>`, an atomically reference-counted owner used when tasks or cloned envelopes share one allocation.
- **Async function**: A function declared with `async` whose future can suspend at `.await`.
- **Borrow**: Temporary access through a reference such as `&Config` without taking ownership.
- **Bounded channel**: A queue with a fixed capacity, such as Tokio `mpsc::channel(capacity)`.
- **Crate**: A Rust compilation unit. In this workspace a crate may be a library or binary target.
- **Enum**: A closed set of variants, such as `NodeDef`, that callers handle with pattern matching.
- **Move**: Transfer of a value's ownership, such as sending an envelope or moving a node bundle into a task.
- **Ownership**: Rust's rule that a value has an owner responsible for its lifetime and cleanup.
- **Public re-export**: A `pub use` that exposes an item through a crate or module boundary.
- **RAII**: Resource acquisition is initialization; an owned guard restores state in its `Drop` implementation.
- **Result**: `Result<T, E>`, an enum carrying either a success value or a typed error.
- **Trait object**: A value such as `Box<dyn Source + Send>` that stores an unknown concrete type behind a trait interface.
- **Workspace**: A group of Cargo packages that share resolution, dependency declarations, and configuration.

## Status boundaries

**Current implementation:** The cited files use all concepts above. Tests verify configuration loading, accumulated validation failures, shared envelope storage, runner message flow, and RAII cleanup behavior.

**Intended design:** These Rust mechanisms reinforce ownership boundaries: data types do not perform I/O, messages move through bounded queues, adapters are owned by their tasks, and guards pair state changes with cleanup.

**Known drift:** Names such as "zero-copy envelope" can overstate the whole path. The host envelope shares header and payload storage across Rust clones, but crossing WIT still marshals data according to the Component Model bindings. The configured edge overflow value is initially stored in `EdgeSender`, but `collect_downstream_senders` does not copy it into `DownstreamSender`; `send_one` waits on `sender.reserve()`. The current downstream path therefore does not dispatch the configured `Drop` or `DeadLetter` behavior described by broader documentation. Consult the [WIT contract reference](../interfaces/wit-contracts.md) for the guest boundary, and verify host behavior in `node/wasm.rs`.

## Evidence

- **Source:** [`Cargo.toml`](../../Cargo.toml) | symbols: `[workspace]`, `members = [`
- **Source:** [`crates/wafer-config/src/lib.rs`](../../crates/wafer-config/src/lib.rs) | symbols: `pub use loader::load_config`, `pub use validation::validate`
- **Source:** [`crates/wafer-config/src/loader.rs`](../../crates/wafer-config/src/loader.rs) | symbols: `pub fn load_config`, `Result<Config, ConfigError>`
- **Source:** [`crates/wafer-types/src/config/mod.rs`](../../crates/wafer-types/src/config/mod.rs) | symbols: `pub enum NodeDef`, `pub const fn category`
- **Source:** [`crates/wafer-core/src/node/traits.rs`](../../crates/wafer-core/src/node/traits.rs) | symbols: `pub trait Lifecycle`, `pub trait Transform`, `Box<dyn Future`
- **Source:** [`crates/wafer-core/src/queue/envelope.rs`](../../crates/wafer-core/src/queue/envelope.rs) | symbols: `pub struct RuntimeEnvelope`, `Arc<EnvelopeHeader>`, `pub payload: Bytes`
- **Source:** [`crates/wafer-core/src/node/state.rs`](../../crates/wafer-core/src/node/state.rs) | symbols: `pub struct ProcessingGuard`, `impl Drop for ProcessingGuard`
- **Source:** [`crates/wafer-core/src/orchestrator/builder.rs`](../../crates/wafer-core/src/orchestrator/builder.rs) | symbols: `fn collect_downstream_senders`, `overflow: edge.overflow.unwrap_or_default()`
- **Source:** [`crates/wafer-core/src/runner/mod.rs`](../../crates/wafer-core/src/runner/mod.rs) | symbols: `async fn send_one`, `sender.sender.reserve().await`
- **Test:** [`crates/wafer-core/src/queue/envelope.rs`](../../crates/wafer-core/src/queue/envelope.rs) | symbols: `fn test_clone_shares_header_via_arc()`, `fn test_clone_shares_payload_bytes()`
- **Test:** [`crates/wafer-core/src/runner/source.rs`](../../crates/wafer-core/src/runner/source.rs) | symbol: `async fn test_source_loop_messages_flow()`
- **Test:** [`crates/wafer-config/src/validation.rs`](../../crates/wafer-config/src/validation.rs) | symbol: `fn test_accumulated_errors()`
- **Test:** [`crates/wafer-core/src/node/state.rs`](../../crates/wafer-core/src/node/state.rs) | symbol: `fn test_processing_guard_clears_on_panic()`
