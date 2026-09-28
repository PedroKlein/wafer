# Trace a stateless hot-swap

> **Documentation type:** Tutorial
>
> **Prerequisites:** [Cross the plugin boundary](plugin-boundary.md) and [Shutdown and failure behavior](shutdown-and-failure.md).

## Purpose

Follow replacement preparation, watch-channel delivery, between-message application, completion reporting, and the bounded process-time recovery path. The walkthrough distinguishes behavior proved by source and tests from broader rollback designs.

## Prerequisites

Use a Transform node for the complete walkthrough because current process-time rollback is implemented there. Filter and Router support replacement and between-message application, but the Transform runner owns the canary rollback logic below. This walkthrough follows source at commit `f173151a8951736b4d82e10ce2b1c4417b02cf99`.

## Flow

```mermaid
sequenceDiagram
    participant API as API handler
    participant Engine as WaferEngine
    participant Watch as watch channel
    participant Runner as Transform runner
    participant Guest as New guest instance
    API->>Engine: compile, pre-instantiate, instantiate
    Engine-->>API: SwapPayload with new Store
    API->>Watch: send replacement
    Runner->>Watch: borrow_and_update between messages
    Runner->>Guest: validate and init
    Guest-->>Runner: replacement_adopted
    Runner->>Guest: process next message
    alt first local outcome completes
        Runner-->>API: first_post_replacement_local_outcome
    else process traps in canary window
        Runner->>Runner: re-instantiate prior pre-instance
        Runner-->>API: rolled_back outcome
    end
```

1. During pipeline construction, each Transform, Filter, and Router bundle receives `watch::channel(None)`. Sources and sinks receive no swap channel.
2. The API acquires a per-node swap guard, reads replacement bytes, and selects the preparation function for the node category. `prepare_transform_swap_timed` compiles through the engine cache, pre-instantiates the typed world, creates a new Store and bindings, and packages them in `SwapPayload::Transform` with `HotSwapProgress`.
3. `PipelineHandle::send_swap` publishes the payload through the node's watch sender. The transform runner polls `swap_rx.has_changed()` before selecting another message, and its input wait also wakes on `swap_rx.changed()`, so an idle node swaps at once. Replacement happens between guest calls rather than by cancelling one.
4. `SwapPayload::try_apply_transform` takes ownership of the prepared Store and bindings. `WasmTransformNode::try_hot_swap` temporarily retains the old Store, bindings, and pre-instance, runs the replacement's `validate_and_init`, and keeps the old values only if that initialization fails.
5. On success, the runner marks `replacement_adopted`, records a swap, and retains the prior `InstancePre` during a bounded canary window. The first forwarded/enqueued, filter-dropped, or router-no-route result completes `first_post_replacement_local_outcome`.
6. If the new Transform traps while that canary is active, the runner restores the prior cached pre-instance and calls `transform.recover_from_cached_pre()`. This creates another fresh Store, re-instantiates prior code, runs lifecycle initialization, retries the trapped envelope, and records recovery and rollback metrics. It reports `rolled_back` only while the caller is waiting; a later rollback cannot rewrite an already completed local-outcome response.

## Rust

A Tokio watch channel stores the latest `Option<SwapPayload>`. The payload is cloneable because the single-consumer Store and binding values sit in `Arc<Mutex<Option<T>>>`; `Option::take` transfers each prepared value exactly once into the owning runner task.

The live node is not shared behind a hot-path mutex. The runner owns it. `std::mem::replace` provides an initialization-failure transaction: the candidate becomes active for validation, and the old host objects can be put back if validation or initialization fails.

`HotSwapProgress` combines `OnceLock` timestamps with a oneshot result. Adoption alone does not complete the response. The first runner-local outcome supplies the second marker. Before that marker consumes the sender, a process-time rollback completes the pending caller with `rolled_back`. After a successful local outcome has already returned, a later canary rollback cannot rewrite the response; recovery state and metrics are then the remaining local record.

## Design

Hot-swap is stateless replacement. Mutable guest state is lost whenever a new Store and instance replace the old ones. The prior `InstancePre` retained for process-time rollback contains reusable compiled and linked code, not a snapshot of guest memory, globals, handles, or thread-local plugin state.

There are two different failure behaviors. Failed `validate` or `init` restores the still-retained old Store and bindings inside `try_hot_swap`. A later transform `process` trap invokes configured canary recovery by instantiating the prior pre-instance into a fresh Store. Neither path retains mutable state from the discarded guest instance.

The documented node-state tracker has Error, Recovering, and Running transitions during process-time recovery. There is no separate rollback node-state transition. Do not invent one from the `rolled_back` API status or rollback metric.

## Status boundaries

**Current implementation:** Watch senders exist only for loaded Wasm processing nodes; replacement payloads are prepared for Transform, Filter, and Router; runner loops apply them between messages. The Transform runner additionally implements configured canary recovery. Hot-swap and reconfigure share one per-node mutation guard.

**Intended design:** The API and metrics divide preparation, signal, replacement adoption, first runner-local outcome, rollback, and recovery so each claim has one owner. None is sink evidence; sink transition, sequence continuity, loss, throughput, and gap require evaluation artifacts.

**Known drift:** Current builder comments call every configured processing category a Wasm node even though Transform and Filter can select native baseline functions. Those native variants reject a Wasm swap. Also, only the transform runner establishes process-time canary rollback; do not generalize it to Filter or Router. The integration tests are conditional on prebuilt component fixtures.

## Evidence

- **Source:** [`crates/wafer-core/src/orchestrator/builder.rs`](../../crates/wafer-core/src/orchestrator/builder.rs) | symbols: `watch::channel(None)`, `watch_senders.insert`
- **Source:** [`crates/wafer-core/src/orchestrator/hotswap.rs`](../../crates/wafer-core/src/orchestrator/hotswap.rs) | symbols: `pub async fn prepare_transform_swap_timed`, `SwapPayload::Transform`
- **Source:** [`crates/wafer-core/src/orchestrator/pipeline.rs`](../../crates/wafer-core/src/orchestrator/pipeline.rs) | symbols: `pub fn send_swap`, `pub fn record_hotswap_phase`
- **Source:** [`crates/wafer-core/src/runner/mod.rs`](../../crates/wafer-core/src/runner/mod.rs) | symbols: `pub struct HotSwapProgress`, `pub fn report_rolled_back`, `pub enum SwapPayload`
- **Source:** [`crates/wafer-core/src/runner/transform.rs`](../../crates/wafer-core/src/runner/transform.rs) | symbols: `swap_rx.has_changed()`, `transform.recover_from_cached_pre()`, `metrics.record_rollback()`
- **Source:** [`crates/wafer-core/src/node/wasm.rs`](../../crates/wafer-core/src/node/wasm.rs) | symbols: `pub async fn try_hot_swap`, `self.store = old_store`, `pub async fn recover_from_cached_pre`
- **Source:** [`crates/wafer-core/src/node/metrics.rs`](../../crates/wafer-core/src/node/metrics.rs) | symbols: `pub fn record_swap`, `pub fn record_rollback`, `pub fn record_recovery`
- **Source:** [`crates/wafer-core/src/api/handlers.rs`](../../crates/wafer-core/src/api/handlers.rs) | symbols: `pub async fn hot_swap`, `"status": "rolled_back"`, `"replacement_adopted": true`
- **Test:** [`crates/wafer-core/tests/hotswap_process_time_rollback.rs`](../../crates/wafer-core/tests/hotswap_process_time_rollback.rs) | symbols: `async fn hotswap_process_time_rollback()`, `async fn hotswap_bounded_rollback_thrash()`

## Checkpoint

Locate the point where `swap_rx.has_changed()` is checked. List what a full replacement transfers, what survives only as reusable host-side code/configuration, and what mutable guest state disappears. Finally, explain why `rolled_back` is an API outcome rather than a new `NodeState` variant.
