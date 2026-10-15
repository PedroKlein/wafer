# Tokio in WAFER's context

> **Read first:** [Rust in WAFER's context](rust-in-context.md).

This page explains the Tokio primitives under the runners and the orchestrator and ties each one to the WAFER code that uses it. By the end you will know which work runs as a Tokio task and which on its own thread, how bounded channels carry backpressure, how a node waits for input, a swap, a retry, or cancellation in one `select!`, and why the guest call is the one wait that cancellation never interrupts. [Shutdown and failure behavior](shutdown-and-failure.md) and [Trace a stateless hot-swap](stateless-hot-swap.md) later describe what those code paths do. `CancellationToken` comes from the `tokio-util` crate; everything else is in `tokio` itself.

## The runtime and its tasks

`main` in `crates/wafer-runtime/src/main.rs` builds the runtime by hand with `tokio::runtime::Builder::new_multi_thread().enable_all()` and then calls `block_on` on the `run` future. `enable_all` turns on the I/O driver, which sockets and signals need, and the timer driver, which `sleep` and `timeout` need. The builder never calls `worker_threads`, so Tokio starts one worker thread per CPU unless the `TOKIO_WORKER_THREADS` environment variable sets another count. `provenance_json` in `crates/wafer-runtime/src/metadata.rs` records the count a run actually had as `tokio_worker_threads`.

A task is a future that the runtime owns and polls until it completes. When a poll reaches an `.await` whose operation is not ready, the future returns `Pending` and the worker thread moves on to another task. The task is polled again when its waker fires, possibly on a different worker. Nothing preempts a task between two `.await` points.

That last rule shapes WAFER. A guest call such as `transform.process(envelope).await` in `crates/wafer-core/src/runner/transform.rs` runs guest code on the worker thread that polls it. Until the guest returns or waits on an asynchronous host import, that worker runs nothing else. A guest that spins with no fuel or epoch limit can hold a worker indefinitely, which is why the epoch ticker and the signal listener run on their own OS threads.

| Work | How it starts | Code |
|---|---|---|
| One task per node | `JoinSet::spawn` | `crates/wafer-core/src/orchestrator/pipeline.rs` (`spawn_bundles`, `spawn_node_task`) |
| DLQ writer | `tokio::spawn`, handle kept in `dlq_handle` | `crates/wafer-core/src/orchestrator/pipeline.rs` (`from_build_output`) |
| API and metrics servers, bench samplers, timed swap trigger | `tokio::spawn` | `crates/wafer-runtime/src/main.rs` (`launch_control_plane`, `run`) |
| Swap compile and link, plugin hashing | `tokio::task::spawn_blocking` | `crates/wafer-core/src/orchestrator/hotswap.rs` (`compile_and_link`), `crates/wafer-core/src/api/handlers.rs` (`hot_swap`) |
| Epoch ticker | `std::thread` | `crates/wafer-core/src/engine/loader.rs` (`ensure_epoch_ticker`) |
| Signal listener | `std::thread` with its own current-thread runtime | `crates/wafer-runtime/src/main.rs` (`spawn_shutdown_signal_thread`) |
| Bench pacer | `std::thread` feeding an `mpsc` channel with `blocking_send` | `crates/wafer-core/src/node/source/bench.rs` (`Pacer::start`) |

## Spawning: JoinSet and Send + 'static

`PipelineOrchestrator` keeps every node task in one `JoinSet<NodeTaskResult>`. `run_until_complete` calls `join_next_with_id`, which returns whichever task finishes next together with its Tokio task `Id`, and the `task_nodes` map turns that `Id` back into a node name for the failure report. A task that panicked comes back as a `JoinError`: the release profile keeps `panic = "unwind"`, and Tokio catches the panic at the task boundary. At the shutdown deadline, `abort_stuck_tasks` calls `JoinSet::shutdown`, which aborts the remaining tasks and waits for them to stop.

`spawn_node_task` wraps each node task in an outer `async move` block that holds a drop guard of the pipeline token and disarms it only when the task returns `Ok`, so one failed node stops the whole run. The DLQ writer is not in the `JoinSet`. It drains until the last node drops its DLQ sender, so `run_until_complete` waits for its `JoinHandle` only after the node tasks are joined, with its own 5 s timeout.

`JoinSet::spawn` and `tokio::spawn` require `Future + Send + 'static`. `Send` is needed because the task may resume on another worker thread after any `.await`. `'static` is needed because the task can outlive the function that spawned it, so the future may not borrow from that function's stack. WAFER meets both bounds by [moving](rust-in-context.md#move) owned values into `async move` blocks: in `spawn_bundles`, each bundle's receiver, senders, watch receiver, error policy, child token, and node move into the runner future, and values that the orchestrator and the API also read, such as `NodeStateTracker` and `NodeMetrics`, are shared through [`Arc`](rust-in-context.md#arc). The node's Wasmtime `Store` moves into its one task; the test `inference_state_keeps_store_send` in `crates/wafer-core/src/engine/state.rs` checks at compile time that `Store<WaferState>` is `Send`.

## spawn_blocking for synchronous work

`compile_and_link` in `crates/wafer-core/src/orchestrator/hotswap.rs` runs Wasmtime compile and link through `tokio::task::spawn_blocking`. Both are synchronous CPU work; on a worker thread they would stall every node task scheduled on that worker. `spawn_blocking` runs the closure on a separate pool of threads that Tokio keeps for blocking work and returns a `JoinHandle` that async code can await. The `??` after `.await` unwraps two layers: a `JoinError` if the closure panicked, then the compile or link error itself. The `hot_swap` handler hashes the plugin bytes the same way. At startup, the launcher compiles on the async path, because no node task exists yet to wait behind it.

## Channels

### Bounded mpsc between nodes

`wire_queues` (`crates/wafer-core/src/orchestrator/builder.rs`) creates one [bounded](rust-in-context.md#bounded-channel) `mpsc::channel(capacity)` per destination node. The capacity is the largest explicit `capacity` among that node's incoming edges, or `engine.default_queue_capacity` (1024 by default) when none sets one. Each upstream edge gets a clone of the destination's `Sender`, which is how several producers feed one receiver. Tokio panics on a capacity of 0, so `validate_channel_capacities` rejects zero before any channel exists.

`send_one` in `crates/wafer-core/src/runner/mod.rs` picks the send call from the edge's overflow policy:

- `slow`, the default, calls `sender.reserve().await`. When the queue is full this future waits until the receiver frees a slot, which is how backpressure reaches the producer. It returns a `Permit`, and `permit.send(envelope)` then neither waits nor fails. `reserve` returns `Err` once the receiver has been dropped.
- `drop` and `dead-letter` call `try_send`, which never waits. `TrySendError::Full` and `TrySendError::Closed` hand the envelope back, so the code can count it or wrap it for the DLQ.

The error policy also writes to the DLQ with `try_send`, so a node never waits for the DLQ. `send_matching` sends to the `try_send` edges before the `slow` ones, so a full slow edge cannot delay a drop edge, and it moves the envelope into the last send instead of cloning it.

End of input is a channel event. Once every `Sender` of a channel has been dropped and its buffer is empty, `recv()` returns `None`. A node task drops its senders when it returns, so end of input travels downstream one channel at a time.

### watch for hot-swap

`build_pipeline_inner` gives every Transform, Filter, and Router bundle a `watch::channel(None)`. A watch channel stores one value and a version number, and each receiver remembers the last version it has seen, so a reader only ever sees the latest payload. In `crates/wafer-core/src/runner/mod.rs`, `take_pending_swap` checks for an unseen version without waiting and takes it with `borrow_and_update()`, and `recv_next_or_retry` awaits `swap_rx.changed()` as a `select!` branch, so an idle node wakes for a swap. `changed()` marks the version seen when it returns, so the code calls `mark_changed()` to re-arm it for `take_pending_swap` at the top of the loop.

### oneshot and Notify for completion

A oneshot channel carries one value from one sender to one receiver; the receiver gets `Err` if the sender is dropped without sending. `HotSwapProgress::channel` creates one per replacement request and keeps the sender in a `std::sync::Mutex<Option<oneshot::Sender<_>>>`, so whichever of `report_init_failed`, `report_rolled_back`, or `try_complete` runs first takes it and sends. The `hot_swap` handler waits with `tokio::time::timeout(REPLACEMENT_OUTCOME_WAIT, &mut completion_rx)`; passing `&mut` keeps the receiver alive when the 5 s timeout fires, so the handler can keep waiting on it.

`HotSwapProgress` also holds a `tokio::sync::Notify` for a caller that waits only for adoption. `mark_replacement_adopted` sets a timestamp and calls `notify_one`, which stores a permit when no task is waiting yet, so a notification sent between the waiter's check and its `notified().await` is not lost.

## select! and cancel safety

`tokio::select!` polls several futures on the current task, runs the handler of the first one that completes with a value matching its pattern, and drops the other branch futures. With `biased;` it polls the branches top to bottom instead of in random order. The idle wait in `recv_next_or_retry` (`crates/wafer-core/src/runner/mod.rs`) is the select every processing node spends most of its time in:

```rust
tokio::select! {
    biased;
    () = &mut cancelled => return NextInput::Closed,
    message = receiver.recv() => Some(message),
    Ok(()) = swap_rx.changed() => None,
}
```

A second form of the same select adds `() = tokio::time::sleep_until(deadline) => continue` when a retry is waiting in backoff. The source and processing loops put cancellation first; `run_sink_loop` puts `receiver.recv()` first, since it drains its buffer after cancellation anyway.

Because the losing futures are dropped, every branch future must be cancel safe: dropping it before it completes must lose nothing. `mpsc::Receiver::recv` guarantees that no message was taken if it loses, and `watch::Receiver::changed` that no version was marked seen. `cancelled()` and `sleep_until` hold no data. `Source::poll` depends on the adapter: the module comment in `crates/wafer-core/src/runner/source.rs` warns that a cancelled poll may drop a partly read item.

Two rules follow from how the macro works. First, a handler body is not a cancellation point. In `run_source_loop`, `send_downstream(&senders, envelope).await` runs inside the `source.poll()` handler, so a source waiting on a full `slow` edge does not see cancellation until a slot frees or the downstream task exits and drops its receiver. Second, a branch whose pattern does not match is disabled for the rest of that `select!` call, so `Ok(()) = swap_rx.changed()` switches itself off when the watch sender is gone instead of ending the wait.

Each loop creates its cancellation future once, with `let cancelled = cancel.cancelled();` and `tokio::pin!(cancelled);`, and passes `&mut cancelled` to every `select!`. A future polled through `&mut` must be `Unpin`, and `tokio::pin!` makes it so by pinning it on the stack. Reusing one future keeps its registration in the token's waiter list across messages.

The guest call is the one future WAFER treats as not cancel safe. Dropping a Wasmtime call future midway would leave the guest stopped in the middle of a call while the runner still owns its `Store`; the module comment in `crates/wafer-core/src/runner/mod.rs` calls this Store poisoning. The runners therefore obtain an envelope through `select!` and then await `transform.process(envelope)` as a plain statement; the Filter and Router loops do the same with their own calls. The call completes, and the loop checks cancellation on its next pass. Fuel and epoch limits, when configured, bound how long such a call can run; cancellation does not.

## CancellationToken

`build_pipeline_inner` creates one `CancellationToken` for the pipeline and gives every node bundle `cancel_token.child_token()`. Cancelling a token cancels all of its children, but cancelling a child would not cancel its parent; no production code cancels a single child. These callers cancel the pipeline token or a clone of it:

- the signal thread, on the first SIGINT or SIGTERM;
- the API `shutdown` handler, through `PipelineHandle::cancel`;
- `PipelineOrchestrator::shutdown` and `PipelineOrchestrator::cancel`;
- `spawn_node_task`, when a node task returns `Err` or panics.

Cancellation only wakes the futures returned by `cancelled()`; it interrupts nothing. Tasks leave their loops at their next `select!`, and the 5 s `JoinSet::shutdown` fallback handles the rest.

## Time

Tokio's timers run on the runtime's timer driver and measure `tokio::time::Instant`. `recv_next_or_retry` races input against `sleep_until(deadline)`, where the deadline is the earliest one among the buffered retries; it is absolute, so building a new timer on each pass does not move it. `run_until_complete` creates `sleep(SHUTDOWN_TIMEOUT)` once, pins it, and polls `&mut deadline`, so the 5 s window does not restart each time a task finishes. `tokio::time::timeout(duration, future)` returns `Err(Elapsed)` if the future has not finished in time.

Because `ErrorPolicyExecutor` stores retry times as `tokio::time::Instant`, tests under `#[tokio::test(start_paused = true)]` can move the clock with `tokio::time::advance` and check backoff and the shutdown deadline without waiting in real time.

## Threads outside the runtime

`spawn_shutdown_signal_thread` builds a separate current-thread runtime with its own I/O driver and runs the SIGTERM and SIGINT listeners on it. The first signal cancels the pipeline token; the second calls `std::process::exit` with 143 or 130. Because this runtime runs on its own thread, a guest that occupies every main worker cannot keep the signal from being seen.

The `wafer-epoch-ticker` thread sleeps with `std::thread::sleep` between ticks, because a Tokio task doing the same job would never be polled while every worker is inside a guest call; [Wasmtime in context](wasmtime-in-context.md#the-engine) describes the ticker itself. The `bench-pacer` thread from `Pacer::start` goes the other way: it hands sequence numbers to the async bench source through `blocking_send`, the synchronous form of `send` for code outside the runtime.

## Locks and atomics

Core code uses `std::sync::Mutex` and `RwLock` only for short critical sections that never span an `.await`, such as the take-once `Arc<Mutex<Option<T>>>` slots in `SwapPayload`. The workspace Clippy lint `await_holding_lock = "deny"` in the root `Cargo.toml` rejects a std guard held across `.await`, which would also make the future non-`Send`. `tokio::sync::Mutex`, whose guard may be held across `.await`, appears outside tests only around the bench recorders in `crates/wafer-runtime/src/main.rs`.

Atomics replace locks where one value is enough. [Rust in context](rust-in-context.md#arc-atomics-and-raii) explains the `Relaxed` metric counters and the `compare_exchange` claim in `PipelineHandle::try_begin_swap`. `HotSwapProgress::try_claim` and `try_withdraw` race on one `AtomicU8` in the same way, so the runner and the API handler cannot both own a payload.

## Limits to keep in mind

- `StdinSource::poll` (`crates/wafer-core/src/node/source/stdin.rs`) calls the blocking `read_line` from `std::io` inside its future. While it waits for a line it holds a worker thread, and its loop cannot see cancellation until a line or end of input arrives ([#235](https://github.com/PedroKlein/wafer/issues/235)).
- The release-profile comment in the root `Cargo.toml` says node failures are contained with `catch_unwind`. No production code calls it; Tokio's task harness catches panics and the `JoinSet` reports them.

## Check yourself

**1. Why must a node task be `Send + 'static`, and how does `spawn_bundles` satisfy that?**

<details markdown="1">
<summary>Answer</summary>

The task may resume on another worker thread after any `.await` (`Send`) and may outlive the function that spawned it (`'static`). `spawn_bundles` moves each bundle's owned parts (receiver, senders, watch receiver, error policy, child token, and node with its `Store`) into an `async move` block, and shares `NodeStateTracker` and `NodeMetrics` through `Arc`.

</details>

**2. Why is `transform.process(envelope)` never a branch of the `select!` in `recv_next_or_retry`?**

<details markdown="1">
<summary>Answer</summary>

`select!` drops the losing branches, and dropping a Wasmtime call future midway would leave the guest stopped mid-call while the runner still owns its `Store`. The runner takes an envelope through the `select!`, whose branches are cancel safe, and then awaits the guest call as a plain statement.

</details>

**3. Why do the epoch ticker and the signal listener run on their own OS threads rather than as Tokio tasks?**

<details markdown="1">
<summary>Answer</summary>

A guest call runs on the worker that polls it and is never preempted. If guests occupied every worker, a Tokio task would never be polled, so the ticker could not end a runaway call and a signal would not be seen.

</details>

## Next

[Configuration to running pipeline](config-to-running-pipeline.md) follows a TOML file to the tasks described here.
