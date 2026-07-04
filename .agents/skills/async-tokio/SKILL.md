---
name: async-tokio
description: >
  Async Rust and Tokio patterns for WAFER's DAG pipeline runtime. Covers cancel safety
  and cancel correctness (including the WASM Store poisoning exception), backpressure with
  bounded channels, graceful shutdown via CancellationToken, select! loop pitfalls,
  Sender::reserve for cancel-safe sends, runtime tuning for edge hardware, and
  drain-and-flip async coordination. Use when writing or reviewing async code in the
  runtime: node loops, hot-swap coordination, channel wiring, shutdown sequences, or
  task lifecycle management. Triggers on: select!, CancellationToken, channel, backpressure,
  drain, shutdown, spawn, JoinHandle, timeout, async, cancel safety, mpsc, bounded queue,
  runtime, worker threads. Do NOT use for general Rust idioms (use rust-best-practices)
  or WASM API patterns (use wasm-specialist).
---

# Async Tokio Patterns for WAFER

## Pre-flight: Async Node Code Checklist

Before writing or reviewing async node code, verify:
- [ ] No WASM call inside a `select!` branch (Store poisoning — not just data loss)
- [ ] Every channel is bounded with documented capacity reasoning
- [ ] Every `select!` branch: is it cancel-safe? (Oxide RFD-400 test: "if dropped mid-await, is state corrupted?")
- [ ] Shutdown closes nodes in **reverse topo order** (sinks drain before sources stop)
- [ ] Every drain poll is wrapped in `tokio::time::timeout`
- [ ] Every `JoinHandle` is awaited — never dropped, never `.abort()`ed

---

## Cancel Safety vs Cancel Correctness (Oxide RFD-397/400)

**Cancel safety** is a LOCAL property: a future can be dropped at any await point without
data loss or invariant violation.

**Cancel correctness** is a GLOBAL property: the system behaves correctly considering
that futures within it might be cancelled.

A cancel correctness bug requires ALL THREE:
1. A cancel-unsafe future exists in the system
2. That future actually gets cancelled (via select!, timeout, abort, runtime shutdown)
3. Cancelling it violates a system property (data loss, invariant corruption, resource leak)

### WASM: Beyond Cancel Safety — Store Poisoning

WASM calls are worse than cancel-unsafe — they **corrupt the instance permanently**:
```rust
// CATASTROPHIC — not just data loss, the Store is permanently poisoned
tokio::select! {
    result = instance.call_process(&envelope) => { /* ... */ }
    _ = cancel_token.cancelled() => { return; }  // Drops mid-WASM → Store unusable
}
```

Normal cancel-unsafe futures (like `Sender::send`) lose data but remain usable.
A cancelled WASM call makes the entire instance produce undefined behavior on
subsequent calls (wasmtime #10088, #10995).

### Cancel-Safe Send Pattern: Sender::reserve

`mpsc::Sender::send(value)` is NOT cancel-safe — if dropped mid-await, the value is lost.
Use `Sender::reserve()` to split the async wait from the synchronous send:

```rust
// BAD — value lost if interval.tick() wins the select!
tokio::select! {
    _ = sender.send(envelope) => {}
    _ = interval.tick() => { log_slow(); }
}

// GOOD — reserve is cancel-safe (you just lose your place in line)
tokio::select! {
    permit = sender.reserve() => {
        let permit = permit?;
        permit.send(envelope);  // Synchronous — cannot be cancelled
    }
    _ = interval.tick() => { log_slow(); }
}
```

For WAFER's overflow policies: `Slow` uses `.send().await` outside select (no cancel risk).
`Drop` and `DeadLetter` use `try_send()` (synchronous — no cancel risk).
The cancel hazard is only when mixing sends with other futures in select!.

---

## Select! Loop Structure

```rust
loop {
    let envelope = tokio::select! {
        biased;  // Deterministic: check cancellation first
        _ = cancel_token.cancelled() => break,
        msg = receiver.recv() => match msg {
            Some(e) => e,
            None => break,  // Upstream channel closed
        },
    };
    
    // WASM processing OUTSIDE select — runs to completion, cannot be cancelled
    state_tracker.set_processing(true);
    let result = node.process(&envelope).await;
    state_tracker.set_processing(false);
    
    sender.send(output).await.ok();  // Backpressure point — blocks if full
}
```

**Why `biased;`**: Without it, tokio randomly selects among ready branches. Under sustained
load where both branches are always ready, cancellation may never be checked. `biased;`
guarantees cancellation is checked every iteration.

**Why `recv()` is safe in select!**: `mpsc::Receiver::recv` is documented cancel-safe —
if dropped, the message stays in the channel for the next poll.

---

## Backpressure: Channel Sizing as Architecture

The channel capacity is the **slack budget** between producer and consumer — how much
burst the channel absorbs before backpressure propagates upstream. (Meridian Space)

### Sizing Discipline

Every channel capacity must be documented with WHY:
```rust
let queue = BoundedQueue::new(32);  // 32: max burst from MQTT QoS 1 redelivery window
let queue = BoundedQueue::new(1024); // 1024: file source reads in 4KB blocks × 256 batch
```

Rules from production systems:
- **Capacity = expected_burst_size / consumer_processing_rate × safety_margin**
- Too small: unnecessary backpressure oscillation (producer blocks/unblocks rapidly)
- Too large: masks slowness, delays backpressure signal, wastes memory
- Powers of 2 are a cargo cult — size to your actual burst profile
- **Monitor fill level**: a queue that's always >80% full IS the bottleneck

### End-to-End Propagation

Backpressure only works if EVERY edge in the DAG is bounded. One unbounded channel
breaks the entire propagation chain — the unbounded channel acts as an infinite buffer,
and the stages downstream of it can OOM while stages upstream think everything is fine.

---

## Graceful Shutdown

Shutdown in WAFER follows the structured concurrency principle:
1. `cancel_token.cancel()` — signal all node tasks
2. Node loops complete their current WASM call, then break
3. Wait on `JoinSet::join_next()` (NOT sequential handle.await — that serializes on slowest)
4. Close nodes in reverse topo order (sinks flush → transforms drain → sources stop)

### JoinSet over Vec<JoinHandle>

```rust
// BAD — serializes on completion order; can't react to first failure
for handle in handles { handle.await.ok(); }

// GOOD — reacts to whichever finishes first (panics, errors, clean exit)
let mut set = JoinSet::new();
for node in nodes { set.spawn(run_node(node)); }
while let Some(result) = set.join_next().await {
    match result {
        Ok(()) => { /* clean exit */ }
        Err(e) if e.is_panic() => { /* supervisor decides: escalate or continue */ }
        Err(e) => { /* cancelled — expected during shutdown */ }
    }
}
```

### Never Use JoinHandle::abort()

`abort()` cancels the task at the NEXT await point — any await point. You cannot know
what state the task is in. For WASM tasks, this guarantees Store poisoning. Use
`CancellationToken` for cooperative cancellation — the task decides when it's safe to stop.

---

## Runtime Tuning for Edge Hardware

WAFER targets Raspberry Pi 4 / Jetson Orin. Default `#[tokio::main]` is wrong:

```rust
// Default: worker_threads = num_cpus (4 on Pi 4)
// Problem: Tokio's work-stealer doesn't know about priority.
// It will preempt your pipeline loop to service a log flush.

// Better: explicit control
let rt = tokio::runtime::Builder::new_multi_thread()
    .worker_threads(2)           // Leave cores for WASM execution + OS
    .max_blocking_threads(4)     // Limit spawn_blocking pool
    .thread_name("wafer-worker")
    .enable_all()
    .build()?;
```

**Key insight from embedded Rust** (moteDB, 2025): `std::sync::Mutex` beats
`tokio::sync::Mutex` when lock hold time is microseconds. The async mutex has
~2µs scheduling overhead per contention. For WAFER's per-message node state updates,
prefer `std::sync::Mutex` or atomics. Reserve `tokio::sync::Mutex` for operations
that actually need to yield (disk I/O, network calls under the lock).

**tokio::sync::Mutex cancel hazard** (Oxide RFD-397): If a future holding an async
Mutex guard is cancelled, the guard drops — releasing the lock with the invariant
not yet restored. This is how Oxide found state corruption bugs. WAFER avoids this
by holding the node mutex for the entire loop (no cancel point while holding it).

---

## Drain-and-Flip (Thesis RQ3: Hot-Swap Disruption Cost)

RQ3 asks: "What is the disruption cost of replacing a stage at runtime?"
Pass criteria: <100ms pause at p95, zero message loss, <5% throughput dip.
Phases are measured separately: prepare (load+instantiate), drain (wait for queue),
flip (atomic swap), retire (close old). The drain phase typically dominates.

Protocol: `Running → Draining → [drain complete] → Flip → Retired`

Cancel safety design:
- If cancelled during drain: routing re-enables, old node continues (safe)
- After flip: swap is complete regardless (idempotent)
- `Drop` impl on coordinator releases the swap lock unconditionally

```rust
// Drain with hard timeout — swap ALWAYS proceeds (even on timeout)
let timed_out = tokio::time::timeout(self.drain_timeout, async {
    loop {
        if tracker.is_drain_ready() { return; }
        tokio::time::sleep(Duration::from_millis(1)).await;
    }
}).await.is_err();

if timed_out {
    // Proceeding is correct — a stuck node will never drain.
    // Aborting would leave the pipeline permanently un-swappable.
    tracing::warn!("Drain timed out, forcing swap");
}
```

---

## NEVER

- **NEVER put WASM calls inside `select!` branches** — Store poisoning, not just data loss;
  the instance is permanently corrupted (Oxide RFD-397 category: "invariants violated")
- **NEVER use `Sender::send()` inside select! loops** — value is lost on cancellation;
  use `Sender::reserve()` to split async-wait from synchronous-send (Tokio docs, Oxide RFD-400)
- **NEVER use `JoinHandle::abort()`** — cancels at arbitrary await point; impossible to
  reason about what state the task is in; use CancellationToken for cooperative shutdown
- **NEVER use unbounded channels** — breaks end-to-end backpressure propagation; one
  unbounded link allows unbounded memory growth upstream of the actual bottleneck
- **NEVER hold tokio::sync::Mutex across complex await sequences** — cancellation drops
  the guard with invariants unrestored (Oxide found repeated state corruption from this)
- **NEVER use default runtime config on constrained hardware** — `#[tokio::main]` spawns
  num_cpus workers; on Raspberry Pi 4 that's all 4 cores, starving WASM and OS
- **NEVER assume channel close = immediate task stop** — the task finishes its current
  WASM call before observing the closed channel on the next recv()
- **NEVER abort a hot-swap because drain timed out** — a stuck node will never drain;
  aborting leaves the pipeline permanently un-swappable
