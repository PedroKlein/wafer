# ADR-0012: Watch-Channel Between-Messages Hot-Swap

- **Date**: 2026-07-12
- **Status**: Implemented
- **Parent RFC**: [RFC-005](../rfcs/RFC-005-orchestrator.md)
- **Supersedes**: [ADR-0003](0003-hot-swap-mechanism.md) (drain-and-flip)

## Context

[ADR-0003](0003-hot-swap-mechanism.md) defined a four-phase "drain-and-flip" hot-swap algorithm: PREPARE → DRAIN → FLIP → RETIRE. The DRAIN phase stops routing new messages to the old node, waits for in-flight Wasm calls to complete (up to a configurable timeout), then atomically flips. This model assumed a shared-ownership concurrency style where an external `RoutingController` could stop message flow independently of the node's own task.

When RFC-005 redesigned the orchestrator around a task-per-node execution model where each Wasm node **owns** its Store and bindings inside a private `tokio::spawn` task, the shared-ownership prerequisite for drain-and-flip disappeared. No external entity can pause message delivery to a node because the node's own task is the only reader of its input channel. Moreover, because each node loop processes one message at a time (Wasm call runs to completion, then loops back to receive the next message), there is never more than one in-flight call per node at any instant. A "drain" phase that waits for in-flight work to finish is therefore redundant: the node is always idle between iterations of its receive loop.

The orchestrator needed a lighter signalling mechanism that respected the node's exclusive ownership and leveraged the natural message boundary that already exists between loop iterations.

## Decision

We use a per-node `tokio::sync::watch` channel to deliver hot-swap payloads to each Wasm node's runner loop. The channel carries `Option<SwapPayload>`, initialised to `None`. The builder (`crates/wafer-core/src/orchestrator/builder.rs`) creates one `watch::channel(None)` per Transform, Filter, and Router node, stores the sender in `BuildOutput.watch_senders`, and threads the receiver into the node's runner bundle as `swap_rx`. The orchestrator keeps the senders.

Only loaded Wasm nodes are eligible; a native implementation keeps its channel but is rejected before any payload is sent. Hot-swap and reconfigure share one per-node mutation guard, so a second request does not overwrite a pending watch value.

The hot-swap sequence:

1. **PREPARE.** The orchestrator compiles the new component through the shared compiled-component cache and links it on tokio's blocking pool (`spawn_blocking`), so a Cranelift compile never occupies a worker that drives pipeline nodes. It then creates a fresh `Store` with the node's capabilities, memory limit, fuel, and epoch configuration, instantiates the component, and packages everything into a typed `SwapPayload` (`Transform`, `Filter`, or `Router`). Implementation: `crates/wafer-core/src/orchestrator/hotswap.rs` (`prepare_transform_swap_timed`, `prepare_filter_swap_timed`, `prepare_router_swap_timed`).

2. **SIGNAL.** The orchestrator sends `Some(payload)` through the `watch::Sender`. This does not block.

3. **ADOPT.** At the top of each loop iteration (between messages), the runner calls `take_pending_swap`. The input wait (`next_input` / `recv_next_or_retry` in `crates/wafer-core/src/runner/mod.rs`) also selects on `swap_rx.changed()`, so an idle runner wakes for the swap instead of waiting for its next message, and a message dequeued after the signal is held for the replacement. The runner installs the replacement and calls its guest `validate()` and `init()`. If either fails, it restores v1 and reports the failure; v1 keeps running. On success it flushes its retry buffer to the DLQ with reason `hot_swap_drain`, records a swap metric, and continues on the same input channel with the new instance.

4. **RETIRE.** The old `Store` and bindings are dropped when the replacement overwrites them. The runtime does not call the old instance's guest `close()` (nor at shutdown). The replacement runs in a new Store, so guest state starts fresh after every swap.

There is **no explicit drain phase**. Because a Wasm call runs to completion (it is never cancelled mid-flight), there is no in-flight work at the swap point.

**Process-time rollback (Transform only).** After adopting a new component (not a reconfigure), a Transform runner keeps v1's `InstancePre` for a canary window. The window closes after `canary_success_count` successful messages or `canary_window_ms`, whichever comes first (`[engine.hot_swap]`, defaults 32 and 10 s). If v2 traps (fuel and epoch traps included) or returns `unrecoverable` from `process()` inside the window, the runner restores v1 from that `InstancePre` in a fresh Store, at most once per swap. `/metrics` has no rollback counter ([A20](../status/implementation-gaps.md)).

**Config-only change.** `POST /api/v1/nodes/{id}/reconfigure` re-instantiates the node from its cached `InstancePre` with the new config, without recompiling, through the same watch channel and guard.

**Response and timeline.** A successful request reports `replacement_adopted` and `first_post_replacement_local_outcome`. Both are runner-local facts, not sink convergence; sink transition, sequence, throughput, and gap artifacts remain the evidence for external disruption. `SwapTimeline` (`hotswap.rs`) timestamps compile, instantiate, signal, ack (the runner picking up the watch value), and the first post-replacement local outcome, and the response reports which cache tier served the compile.

`NodeState` has no drain-and-flip states: a node is `Starting`, `Running`, `Error` or `Recovering`, and a swap does not change it.

## Consequences

- **Positive: no drain phase.** No drain timeout, no risk of force-draining and losing messages, no `RoutingController`. The swap is atomic at the message boundary.

- **Positive: no per-message cost from hot-swap machinery.** `watch::Receiver::has_changed()` is a single atomic load. The runner owns its instance directly; there is no shared `Mutex<HashMap<…>>` on the hot path.

- **Positive: retry semantics are clear.** On swap, the retry buffer is flushed to the DLQ with reason `hot_swap_drain`, so stale v1 payloads are never replayed against v2 logic.

- **Trade-off: swap latency includes the current message's processing time** plus the replacement's `init()`. It does not depend on the input rate, because the input wait also wakes on the swap signal. Cancelling a Wasm call mid-flight to swap sooner would poison the Store (cancel-safety concern documented in RFC-002).

- **Trade-off: the payload must be fully prepared before signalling.** Compilation and instantiation happen in PREPARE, so adoption on the runner side is short.

- **Forecloses: gradual traffic splitting.** Messages flow through exactly one instance at a time; canary percentages or blue-green dual-instance routing would need a router node upstream.

- **Forecloses: stateful migration.** The old Store is dropped, not drained. Guest state is lost by design; snapshot/restore across versions remains out of scope.

## See Also

- [RFC-005: Orchestrator & Runtime Simplification](../rfcs/RFC-005-orchestrator.md), the long-form decision (§D6) that this ADR summarises.
- [ADR-0003: Drain-and-Flip Hot-Swap](0003-hot-swap-mechanism.md), the superseded mechanism.
- [RFC-002: Host-Side Runtime Architecture](../rfcs/RFC-002-host-runtime.md), which defines the Store-poisoning concern that motivates never cancelling a Wasm call mid-flight.
- `crates/wafer-core/src/orchestrator/hotswap.rs`: `prepare_*_swap_timed` functions and `SwapTimeline`.
- `crates/wafer-core/src/runner/transform.rs`: runner loop with the between-messages swap check and the canary rollback.
