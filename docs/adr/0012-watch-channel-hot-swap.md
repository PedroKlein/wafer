# ADR-0012: Watch-Channel Between-Messages Hot-Swap

- **Date**: 2026-07-12
- **Status**: Accepted
- **Parent RFC**: [RFC-005](../rfcs/RFC-005-orchestrator.md)
- **Supersedes**: ADR-0003 drain phase (§2 DRAIN)

## Context

ADR-0003 defined a four-phase "drain-and-flip" hot-swap algorithm: PREPARE → DRAIN → FLIP → RETIRE. The DRAIN phase stops routing new messages to the old node, waits for in-flight Wasm calls to complete (up to a configurable timeout), then atomically flips. This model assumed a shared-ownership concurrency style where an external `RoutingController` could stop message flow independently of the node's own task.

When RFC-005 redesigned the orchestrator around a task-per-node execution model where each Wasm node **owns** its Store and bindings inside a private `tokio::spawn` task, the shared-ownership prerequisite for drain-and-flip disappeared. No external entity can pause message delivery to a node because the node's own task is the only reader of its input channel. Moreover, because each node loop processes one message at a time (Wasm call runs to completion, then loops back to receive the next message), there is never more than one in-flight call per node at any instant. A "drain" phase that waits for in-flight work to finish is therefore redundant — the node is always idle between iterations of its receive loop.

The orchestrator needed a lighter signalling mechanism that respected the node's exclusive ownership and leveraged the natural message boundary that already exists between loop iterations.

## Decision

We use a per-node `tokio::sync::watch` channel to deliver hot-swap payloads to each Wasm node's runner loop. The channel carries `Option<SwapPayload>`, initialised to `None`. The orchestrator retains the `watch::Sender` for each swappable node; the runner task holds the corresponding `watch::Receiver`.

The hot-swap sequence becomes:

1. **PREPARE** — The orchestrator compiles the new `.wasm` component, pre-instantiates it (`InstancePre`), creates a fresh `Store` with fuel/epoch configuration, and packages everything into a typed `SwapPayload` variant (`Transform`, `Filter`, or `Router`). Implementation: `crates/wafer-core/src/orchestrator/hotswap.rs` (`prepare_transform_swap`, `prepare_filter_swap`, `prepare_router_swap`).

2. **SIGNAL** — The orchestrator sends `Some(payload)` via the `watch::Sender`. This is a single atomic pointer swap inside the watch channel; it does not block.

3. **FLIP** — At the top of the next loop iteration (between messages), the runner checks `swap_rx.has_changed()`. If the watch value is `Some(payload)`, the runner flushes its retry buffer to the DLQ with reason `hot_swap_drain`, applies the payload (replacing its Store, bindings, and cached `InstancePre`), records a swap metric, and continues receiving messages on the same input channel with the new instance.

4. **RETIRE** — The old `Store` and bindings are dropped via normal Rust RAII when the replacement overwrites them. No explicit close call is required.

There is **no explicit drain phase**. The check happens between messages — after the previous Wasm call has returned and before the next `receiver.recv()` call. Because a Wasm call runs to completion (it is never cancelled mid-flight), there is zero in-flight work at the swap point. "Drain time" equals the time to finish processing the current message, which at 1000 msg/s is typically < 1 ms.

This is implemented in all three runner loops:
- `crates/wafer-core/src/runner/transform.rs` — lines 39–47
- `crates/wafer-core/src/runner/filter.rs` — lines 39–47
- `crates/wafer-core/src/runner/router.rs` — lines 39–47

The builder (`crates/wafer-core/src/orchestrator/builder.rs`) creates one `watch::channel(None)` per Wasm node, stores the sender in `watch_senders: HashMap<Box<str>, watch::Sender<Option<SwapPayload>>>`, and threads the receiver into each node's runner bundle via the `swap_rx` field.

## Consequences

- **Positive: Eliminates the drain phase entirely.** No drain timeout, no risk of force-draining and losing messages, no `RoutingController` abstraction. The swap is naturally atomic at the message boundary.

- **Positive: Zero per-message overhead from hot-swap machinery.** `watch::Receiver::has_changed()` is a single atomic load (~1–2 ns). The previous mutex-based approach added lock contention on every message even when no swap was in progress.

- **Positive: No separate shutdown coordination for swap.** Because the node owns its instance, dropping the old Store is automatic. There is no need for a two-phase close sequence during hot-swap.

- **Positive: SwapTimeline phase decomposition simplifies.** The `SwapTimeline` struct (`hotswap.rs`) decomposes latency into compile → instantiate → signal → ack → convergence, with the "ack" being simply the moment the runner picks up the watch value — no drain measurement needed.

- **Positive: Retry buffer semantics are clear.** On swap, the retry buffer is flushed to DLQ with reason `hot_swap_drain`. The new instance starts with a clean retry state. No ambiguity about whether retries should re-execute against the new or old plugin version.

- **Negative / trade-off: Swap latency includes the current message's processing time.** If a node is mid-way through a long Wasm call (e.g., an inference node taking 50 ms), the swap cannot complete until that call finishes. This is acceptable because cancelling a Wasm call mid-flight would poison the Store (cancel-safety concern documented in RFC-002).

- **Negative / trade-off: Single-consumer watch channel means only one pending swap per node.** If the orchestrator sends two swaps rapidly, the second overwrites the first before the node picks it up. This is acceptable because rapid sequential swaps are operationally meaningless — only the latest version matters.

- **Forecloses: Gradual traffic splitting (canary).** Because messages flow through exactly one instance at a time, there is no mechanism to route a percentage of traffic to v2 while v1 handles the rest. Canary deployment would require a router node upstream.

- **Forecloses: Stateful migration.** The old Store is dropped, not drained. Any node-local state accumulated in `WaferState` fields is lost. Stateful hot-swap (snapshot/restore) remains future work.

- **Supersedes ADR-0003 §2 (DRAIN phase).** The original 4-phase algorithm is reduced to 3 phases (PREPARE → SIGNAL/FLIP → RETIRE). The `RoutingController` concept from ADR-0003 is removed from the codebase entirely. ADR-0003 remains as historical context for the higher-level "why hot-swap at all" decision, but its mechanism is superseded by this ADR.

- **Downstream requirement: `SwapPayload` must be pre-built before signalling.** The orchestrator must complete compilation and instantiation *before* sending via the watch channel. This front-loads latency into the PREPARE phase (~8 ms compile + ~5 µs instantiate) but ensures the FLIP is instantaneous from the runner's perspective.

## See Also

- [RFC-005 — Orchestrator & Runtime Simplification](../rfcs/RFC-005-orchestrator.md) — the long-form decision (§D6) that this ADR summarises.
- [ADR-0003 — Hot-Swap Mechanism](0003-hot-swap-mechanism.md) — the original drain-and-flip mechanism that this ADR's watch-channel model supersedes (specifically the DRAIN phase and `RoutingController`).
- [RFC-002 — Host-Side Runtime Architecture](../rfcs/RFC-002-host-runtime.md) — defines the Store-poisoning concern that motivates never cancelling a Wasm call mid-flight.
- `crates/wafer-core/src/orchestrator/hotswap.rs` — `prepare_*_swap` functions and `SwapTimeline`.
- `crates/wafer-core/src/orchestrator/builder.rs` — watch channel creation and wiring.
- `crates/wafer-core/src/runner/transform.rs` — canonical runner loop showing the between-messages swap check.
