# Tokio in WAFER's context

> **Documentation type:** Explanation
>
> **Prerequisites:** [Rust in WAFER's context](rust-in-context.md).

This page collects the Tokio techniques that the runners and the orchestrator depend on and ties each one to the WAFER code that uses it. [Shutdown and failure behavior](shutdown-and-failure.md) and [Trace a stateless hot-swap](stateless-hot-swap.md) describe what those code paths do; this page explains the primitives underneath. `CancellationToken` comes from the `tokio-util` crate; everything else is in `tokio` itself.

## The runtime and its tasks

`main` in `crates/wafer-runtime/src/main.rs` builds the runtime by hand with `tokio::runtime::Builder::new_multi_thread().enable_all()` and then calls `block_on` on the `run` future. `enable_all` turns on the I/O driver, which sockets and signals need, and the timer driver, which `sleep` and `timeout` need. The builder never calls `worker_threads`, so Tokio starts one worker thread per CPU reported by `std::thread::available_parallelism`, unless the `TOKIO_WORKER_THREADS` environment variable sets another count. `provenance_json` in `crates/wafer-runtime/src/metadata.rs` records the count a run actually had as `tokio_worker_threads`. The `waferctl` and `wafer-loadgen` binaries use the `#[tokio::main]` attribute instead.

A task is a future that the runtime owns and polls until it completes. When a poll reaches an `.await` whose operation is not ready, the future returns `Pending` and the worker thread moves on to another task. The task is polled again when its waker fires, possibly on a different worker, because the multi-thread scheduler moves work between threads. Nothing preempts a task between two `.await` points.

That last rule shapes WAFER. A guest call such as `transform.process(envelope).await` in `crates/wafer-core/src/runner/transform.rs` (`run_transform_loop_with_config`) runs guest code on the worker thread that polls it. Until the guest returns or waits on an asynchronous host import, that worker runs nothing else. A guest that spins with no fuel or epoch limit can hold a worker indefinitely, which is why the epoch ticker and the signal listener run on their own OS threads.

| Work | How it starts | Code |
|---|---|---|
| One task per node | `JoinSet::spawn` | `crates/wafer-core/src/orchestrator/pipeline.rs` (`spawn_bundles`, `spawn_wasm_runner`) |
| DLQ writer | `tokio::spawn`, handle kept in `dlq_handle` | `crates/wafer-core/src/orchestrator/pipeline.rs` (`from_build_output`) |
| API and metrics servers, bench samplers, timed swap trigger | `tokio::spawn` | `crates/wafer-runtime/src/main.rs` (`launch_control_plane`, `run`) |
| Swap compile and link, plugin hashing | `tokio::task::spawn_blocking` | `crates/wafer-core/src/orchestrator/hotswap.rs` (`compile_and_link`), `crates/wafer-core/src/api/handlers.rs` (`hot_swap`) |
| Epoch ticker | `std::thread` | `crates/wafer-core/src/engine/loader.rs` (`ensure_epoch_ticker`) |
| Signal listener | `std::thread` with its own current-thread runtime | `crates/wafer-runtime/src/main.rs` (`spawn_shutdown_signal_thread`) |
| Bench pacer | `std::thread` feeding an `mpsc` channel with `blocking_send` | `crates/wafer-core/src/node/source/bench.rs` (`Pacer::start`) |

## Spawning: JoinSet, tokio::spawn, and Send + 'static

`PipelineOrchestrator` keeps every node task in one `JoinSet<NodeTaskResult>`. `run_until_complete` calls `join_next_with_id`, which returns whichever task finishes next together with its Tokio task `Id`. The `task_nodes` map turns that `Id` back into a node name for the failure report. A task that panicked comes back as a `JoinError`: the release profile in the root `Cargo.toml` keeps `panic = "unwind"`, and Tokio catches the panic at the task boundary. At the shutdown deadline, `abort_stuck_tasks` calls `JoinSet::shutdown`, which aborts the remaining tasks and waits for them to stop. Dropping a `JoinSet` would also abort them.

`spawn_wasm_runner` wraps each Transform, Filter, and Router runner in an outer `async move` block before spawning it. When the runner returns `Err`, the wrapper cancels the pipeline token, so one failed processing node stops the whole run. The DLQ writer is not in the `JoinSet`. It is spawned with `tokio::spawn`, and its `JoinHandle` is kept in `dlq_handle`. The writer drains until the last node drops its DLQ sender, so `run_until_complete` waits for it only after the node tasks are joined, with its own 5 s timeout.

`JoinSet::spawn`, `tokio::spawn`, and the `runner` parameter of `spawn_wasm_runner` all require `Future + Send + 'static`. `Send` is needed because the task may resume on another worker thread after any `.await`, so everything the future holds across an `.await` must be safe to move between threads. `'static` is needed because the task can outlive the function that spawned it, so the future may not borrow from that function's stack.

WAFER meets both bounds by [moving](rust-in-context.md#move) owned values into `async move` blocks. In `spawn_bundles`, each bundle's receiver, senders, watch receiver, error policy, child token, and node move into the runner future. Values that the orchestrator and the API also read, such as `NodeStateTracker` and `NodeMetrics`, are shared through [`Arc`](rust-in-context.md#arc). The node's Wasmtime `Store` moves into its one task; the test `inference_state_keeps_store_send` in `crates/wafer-core/src/engine/state.rs` checks at compile time that `Store<WaferState>` is `Send`. Adapter methods such as `Source::poll` (`crates/wafer-core/src/node/source/mod.rs`) return `Pin<Box<dyn Future<...> + Send + '_>>`, which keeps the loop futures that await them `Send`.

`compile_and_link` in `crates/wafer-core/src/orchestrator/hotswap.rs` shows the same bounds on a closure. It receives `engine: &Arc<WaferEngine>` and `node_id: &str`, which are borrows, so it clones the `Arc` and copies the id into an owned `String` before the `move` closure. The Wasm bytes arrive as `Arc<[u8]>`, and the `link` callback is typed `impl FnOnce(&WaferEngine, &Component) -> Result<P> + Send + 'static`.

## spawn_blocking for synchronous work

`compile_and_link` runs Wasmtime compile and link through `tokio::task::spawn_blocking`. A cache miss in `WaferEngine::compile_cached` runs Cranelift, and linking type-checks every import. Both are synchronous CPU work. On a worker thread they would stall every node task scheduled on that worker for the length of the compile. `spawn_blocking` runs the closure on a separate pool of threads that Tokio keeps for blocking work and returns a `JoinHandle` that async code can await. The `??` after `.await` unwraps two layers: first a `JoinError` if the closure panicked, mapped to `WaferError::PluginInit`, then the compile or link error itself. A blocking task cannot be aborted once it has started.

The `hot_swap` handler and the timed swap trigger in `run` also hash the plugin bytes with `spawn_blocking`, so the SHA-256 runs in parallel with swap preparation. At startup, `resolve_and_load_component` (`crates/wafer-core/src/orchestrator/launcher.rs`) calls `compile_cached` directly on the async path. No node task exists yet at that point, so nothing waits behind it.

## Channels

### Bounded mpsc between nodes

`wire_queues` (`crates/wafer-core/src/orchestrator/builder.rs`) creates one [bounded](rust-in-context.md#bounded-channel) `mpsc::channel(capacity)` per destination node. The capacity is the largest explicit `capacity` among that node's incoming edges, or `engine.default_queue_capacity` (1024 by default) when none sets one. Each upstream edge gets a clone of the destination's `Sender`, which is how several producers feed one receiver. Tokio panics on a capacity of 0, so `validate_channel_capacities` rejects zero before any channel exists.

`send_one` in `crates/wafer-core/src/runner/mod.rs` picks the send call from the edge's overflow policy:

- `slow`, the default, calls `sender.reserve().await`. When the queue is full this future waits until the receiver frees a slot, which is how backpressure reaches the producer. It returns a `Permit`, and `permit.send(envelope)` then neither waits nor fails. `reserve` returns `Err` once the receiver has been dropped.
- `drop` and `dead-letter` call `try_send`, which never waits. `TrySendError::Full` and `TrySendError::Closed` hand the envelope back, so the code can count it or wrap it for the DLQ.

The error policy in `crates/wafer-core/src/runner/error_policy.rs` also writes to the DLQ with `try_send`, so a node never waits for the DLQ. `send_matching` sends to the `try_send` edges before the `slow` ones, so a full slow edge cannot delay a drop edge (test `mixed_fan_out_keeps_drop_independent_of_a_slow_sibling`). It moves the envelope into the last send instead of cloning it.

End of input is a channel event. Once every `Sender` of a channel has been dropped and its buffer is empty, `recv()` returns `None`. A node task drops its senders when it returns, so end of input travels downstream one channel at a time.

### watch for hot-swap

`build_pipeline_inner` gives every Transform, Filter, and Router bundle a `watch::channel(None)`. A watch channel stores one value and a version number, and each receiver remembers the last version it has seen. Sending replaces the value, so a reader only ever sees the latest payload. The runner side lives in `crates/wafer-core/src/runner/mod.rs`:

- `take_pending_swap` checks for an unseen version without waiting, then `borrow_and_update()` clones the `Option<SwapPayload>` and marks that version seen.
- `recv_next_or_retry` awaits `swap_rx.changed()` as a `select!` branch, so an idle node wakes for a swap. `changed()` marks the version seen when it returns, so the code calls `mark_changed()` to re-arm it for `take_pending_swap` at the top of the loop.
- `swap_pending` covers a dropped sender. After the last sender is gone, `has_changed()` returns `Err` even when an unseen version is still in the slot, so the function falls back to `borrow().has_changed()`. The tests `swap_wakes_runner_while_input_is_idle` and `unseen_swap_on_closed_channel_is_taken_once` pin this behavior.

### oneshot and Notify for completion

A oneshot channel carries one value from one sender to one receiver. `HotSwapProgress::channel` creates one per replacement request and keeps the sender in a `std::sync::Mutex<Option<oneshot::Sender<_>>>`. Whichever of `report_init_failed`, `report_rolled_back`, or `try_complete` runs first takes the sender and sends; later calls find `None`. The receiver resolves to `Err` if the sender is dropped without sending, which the `hot_swap` handler reports as a runner that exited before adoption.

The handler waits with `tokio::time::timeout(REPLACEMENT_OUTCOME_WAIT, &mut completion_rx)`. Passing `&mut` instead of the receiver keeps the receiver alive when the 5 s timeout fires, so `unfinished_replacement` can still await it. Awaiting `&mut Receiver` is cancel safe.

`HotSwapProgress` also holds a `tokio::sync::Notify` for a caller that waits only for adoption. `replacement_adopted` loops: it returns if the adoption timestamp in a `OnceLock` is set, and otherwise awaits `notified()`. `mark_replacement_adopted` sets the timestamp and then calls `notify_one`, which stores a permit when no task is waiting yet. A notification sent between the check and the wait is therefore not lost.

The timed swap in `run` uses a second oneshot to pass the prepared payload from the trigger task to `run_with_swap`. WAFER does not use `tokio::sync::broadcast`; shutdown fan-out goes through `CancellationToken`.

## select! and cancel safety

`tokio::select!` polls several futures on the current task and runs the handler of the first one that completes with a value matching its pattern. It then drops the other branch futures. Without `biased;` it picks a random branch to poll first on each call; with `biased;` it polls the branches top to bottom. The source, processing, and passthrough loops and `run_until_complete` put cancellation first. `run_sink_loop` puts `receiver.recv()` first, since it drains its buffer after cancellation anyway.

Because the losing futures are dropped, every branch future must be cancel safe: dropping it before it completes must lose nothing. `mpsc::Receiver::recv` guarantees that no message was taken if it loses, `watch::Receiver::changed` that no version was marked seen, and `JoinSet::join_next_with_id` that no task result was removed. `cancelled()`, `sleep`, and `sleep_until` hold no data. `Source::poll` depends on the adapter: the module comment in `crates/wafer-core/src/runner/source.rs` warns that a cancelled poll may drop a partly read item. `MqttSource::poll` only awaits `recv()` on a channel fed by a spawned event-loop task, so it loses nothing.

Two rules follow from how the macro works. First, a handler body is not a cancellation point. Once a branch wins, its handler runs to completion before the loop polls `cancelled` again. In `run_source_loop`, `send_downstream(&senders, envelope).await` runs inside the `source.poll()` handler, so a source waiting on a full `slow` edge does not see cancellation. It continues when a slot frees or when the downstream task exits and drops its receiver. Second, a branch whose pattern does not match is disabled for the rest of that `select!` call. The branch `Ok(()) = swap_rx.changed()` therefore switches itself off when the watch sender is gone instead of ending the wait (test `closed_swap_channel_does_not_end_the_wait`).

Each loop creates its cancellation future once, with `let cancelled = cancel.cancelled();` followed by `tokio::pin!(cancelled);`, and passes `&mut cancelled` to every `select!`. A future polled through `&mut` must be `Unpin`, and `WaitForCancellationFuture` is not, because it holds a registration in the token's waiter list. `tokio::pin!` pins the future on the stack and shadows the name with a `Pin<&mut _>`, which can be polled through `&mut`. Reusing one future keeps the waiter registered across messages instead of building and dropping one on every iteration. The transform loop passes it to `next_input` as `cancelled.as_mut()`, which reborrows the pin without moving it.

The guest call is the one future WAFER treats as not cancel safe. Dropping a Wasmtime call future midway would leave the guest stopped in the middle of a call while the runner still owns its `Store`; the module comment in `crates/wafer-core/src/runner/mod.rs` calls this Store poisoning, and the guest-call methods in `crates/wafer-core/src/node/wasm.rs` are documented as never belonging in a `select!` branch. The runners therefore obtain an envelope through `select!` and then await `transform.process(envelope)` as a plain statement; the Filter and Router loops do the same with their own calls. The call completes, and the loop checks cancellation on its next pass. When they are configured, fuel and epoch limits bound how long such a call can run; cancellation does not.

## CancellationToken

`build_pipeline_inner` creates one `CancellationToken` for the pipeline and gives every node bundle `cancel_token.child_token()`. Cancelling a token cancels all of its children, while cancelling a child would not cancel its parent. A clone, by contrast, is the same token. No production code cancels a single child. These callers cancel the pipeline token or a clone of it:

- the signal thread, on the first SIGINT or SIGTERM;
- the API `shutdown` handler, through `PipelineHandle::cancel`;
- `PipelineOrchestrator::shutdown` and `PipelineOrchestrator::cancel`;
- `spawn_wasm_runner`, when a processing runner returns `Err`;
- `fail_io_init`, when a source or sink `init()` fails.

Cancellation only wakes the futures returned by `cancelled()`; it interrupts nothing. Tasks leave their loops at their next `select!`, and the 5 s `JoinSet::shutdown` fallback handles the rest. The runtime binary keeps two unrelated tokens: `bench_cancel` stops the bench samplers, and `run_done` tells the swap provenance task in `run_with_swap` to stop waiting.

## Time

Tokio's timers run on the runtime's timer driver and measure `tokio::time::Instant`. Three patterns appear:

- `recv_next_or_retry` races input against `sleep_until(deadline)`, where `ErrorPolicyExecutor::next_retry_deadline` gives the earliest deadline among the buffered retries. The deadline is absolute, so building a new timer on each pass does not move it.
- `run_sink_loop` builds `sleep(timeout)` on each pass for batch flushing. Every message restarts the timer, so it fires only after the input has been idle for `timeout`.
- `run_until_complete` creates `sleep(SHUTDOWN_TIMEOUT)` once, pins it with `tokio::pin!`, and polls `&mut deadline` in its inner loop, so the 5 s window does not restart each time a task finishes.

`tokio::time::timeout(duration, future)` returns `Err(Elapsed)` if the future has not finished in time. `run_until_complete` applies it to the DLQ `JoinHandle`. When it expires the handle is dropped, which detaches the task rather than aborting it.

`ErrorPolicyExecutor` stores retry times as `tokio::time::Instant`, not `std::time::Instant`. Under `#[tokio::test(start_paused = true)]` the Tokio clock stands still and moves only through `tokio::time::advance` or when every task is idle. Tests such as `later_earlier_retry_wakes_at_its_own_deadline` in `crates/wafer-core/src/runner/error_policy.rs` and `node_stuck_at_shutdown_is_aborted_and_fails_the_run` in `crates/wafer-core/src/orchestrator/pipeline.rs` check backoff and the 5 s deadline without waiting in real time.

## Threads outside the runtime

`spawn_shutdown_signal_thread` builds a separate runtime with `new_current_thread().enable_io()`. It installs the SIGTERM and SIGINT listeners inside `runtime.enter()`, because `tokio::signal::unix::signal` must register with a runtime's driver. This happens before the thread is spawned, so an install error returns to `run` as an `io::Error`. The `wafer-signals` thread then runs `block_on`. The first signal calls `cancel()` on its clone of the pipeline token; the second calls `std::process::exit` with 143 or 130. This runtime has its own driver on its own thread, so a guest that occupies every main worker cannot keep the signal from being seen.

The `wafer-epoch-ticker` thread from `WaferEngine::ensure_epoch_ticker` sleeps with `std::thread::sleep` between ticks, because a Tokio task doing the same job would never be polled while every worker is inside a guest call; [Wasmtime in context](wasmtime-in-context.md#the-engine) describes the ticker itself.

`Pacer::start` goes the other way. Its `bench-pacer` thread sleeps with `std::thread::sleep` until each message is due and hands the sequence number to the async source through `blocking_send` on a bounded `mpsc` channel. `blocking_send` is the synchronous form of `send` for code outside the runtime; it panics if called from inside an async task.

## Locks and atomics

Core code uses `std::sync::Mutex` and `RwLock` for short critical sections that never span an `.await`: the oneshot sender slot in `HotSwapProgress`, the take-once `Arc<Mutex<Option<T>>>` slots in `SwapPayload`, and the shared plugin-hash map. The workspace Clippy lint `await_holding_lock = "deny"` in the root `Cargo.toml` makes Clippy reject a std guard held across `.await`; such a guard would also make the future non-`Send`. The `lock` helper in `crates/wafer-core/src/runner/mod.rs` recovers a poisoned mutex with `PoisonError::into_inner` instead of panicking.

Outside tests, `tokio::sync::Mutex` appears only in `crates/wafer-runtime/src/main.rs`, around the bench recorders. Each sampler task calls `lock().await` and keeps the guard while `sample_loop` runs for the whole pipeline run. `flush_bench_artifacts` cancels the samplers, joins them, and only then locks a recorder to read its samples. A Tokio guard may be held across `.await`, and `lock()` waits asynchronously instead of blocking a worker.

Atomics replace locks where one value is enough. [Rust in context](rust-in-context.md) explains the `Relaxed` metric counters and the `compare_exchange` claim in `PipelineHandle::try_begin_swap`. `HotSwapProgress::try_claim` and `try_withdraw` use the same kind of claim: they race on one `AtomicU8` with `AcqRel`, so the runner and the API handler cannot both own a payload.

## Working vocabulary

- **Task**: A future owned and polled by the Tokio runtime, started with `tokio::spawn` or `JoinSet::spawn`; it gives up its worker thread only at `.await` points.
- **JoinSet**: A set of spawned tasks that yields results in completion order and aborts the tasks still running on `shutdown` or drop; WAFER keeps one task per node in it.
- **Bounded channel**: An `mpsc` queue with a fixed capacity; `reserve` and `send` wait while it is full, and `try_send` returns `Full` instead.
- **watch channel**: A channel that holds only the latest value and a version number; receivers check or await versions they have not seen. Each processing node receives hot-swap payloads through one.
- **oneshot**: A channel for exactly one value; the receiver gets `Err` if the sender is dropped without sending.
- **Notify**: A Tokio primitive that wakes a waiting task, or stores one permit for the next waiter when nobody is waiting.
- **select!**: A Tokio macro that waits on several futures in one task, runs the handler of the first to complete, and drops the rest; `biased;` fixes the polling order.
- **Cancel safety**: The property that dropping a future before it completes loses no data, which every `select!` branch future needs.
- **CancellationToken**: A `tokio-util` handle whose `cancel()` wakes every `cancelled()` future on that token and on its child tokens.
- **spawn_blocking**: A Tokio function that runs a synchronous closure on a separate thread pool and returns an awaitable `JoinHandle`.

## Status boundaries

**Current implementation:** All node tasks run on one hand-built multi-thread runtime with Tokio's default worker count. Node queues are bounded `mpsc` channels, hot-swap delivery uses `watch`, completion reporting uses `oneshot` and `Notify`, and shutdown uses one pipeline `CancellationToken` with a child per node. Guest calls run outside `select!`. Hot-swap compile and link run on the blocking pool; signals and epoch ticks run on dedicated OS threads.

**Intended design:** Every wait in a runner loop is either a cancel-safe `select!` branch or a guest call that must finish, so cancellation stops a node only between messages, and the bounded `JoinSet` deadline covers a task that does not reach that point.

**Known drift:** `StdinSource::poll` (`crates/wafer-core/src/node/source/stdin.rs`) calls the blocking `read_line` from `std::io` inside its future, so while it waits for a line it holds a worker thread and its loop cannot observe cancellation until a line or end of input arrives. A send blocked on a full `slow` edge is not a cancellation point either. Startup compiles components on the async path, although the `compile_cached` doc comment asks async callers to use the blocking pool. The release-profile comment in the root `Cargo.toml` says node failures are contained with `catch_unwind`; no production code calls it, and panics are caught by Tokio's task harness and reported through the `JoinSet`.

## Checkpoint

1. Why does `spawn_wasm_runner` require `Send + 'static` from its runner, and what does `spawn_bundles` move into the task or share through `Arc` to satisfy it?
2. In `recv_next_or_retry`, which branch futures are cancel safe, why is `mark_changed()` called after `changed()` wins, and why is the guest call not one of the branches?
3. A source is blocked in `send_downstream` on a full `slow` edge when SIGTERM arrives. Which thread sees the signal, which token does it cancel, and why can the source not leave its loop until that send returns?
4. Why do the epoch ticker and the signal listener run on `std::thread` instead of as Tokio tasks, and why does hot-swap compilation use `spawn_blocking`?
