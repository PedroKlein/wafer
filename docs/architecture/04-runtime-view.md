# 04 — Runtime View

> arc42 §6 — Runtime scenarios that illustrate the system's dynamic behaviour.

This section shows how key runtime flows execute inside WAFER. Each scenario
is presented as a sequence diagram. For the static structure these flows
traverse, see [03-building-blocks.md](./03-building-blocks.md).

> **Implementation status.** The scenarios below describe the current
> production behavior. Historical drift banners (A3, A4, A7, A9, A10,
> A13, A14, A15, A17) are closed — local replacement telemetry,
> replacement-time `init()`, production Wasm lifecycle `validate()` / `init()`, source /
> fan-out lineage assignment, per-node-type `/hot-swap` dispatch,
> retry-exhaustion + `Recovering` state transitions,
> capability-aware instantiation, and process-time hot-swap rollback
> (canary window, one rollback per swap, A17 closed 2026-08-02) are all
> wired. See [`../status/implementation-gaps.md`](../status/implementation-gaps.md)
> for the full history; only observability follow-up **A20**
> (Prometheus rollback counter) is still open, and it does not affect
> runtime semantics.

---

## 1. Message Flow: Source → Transform → Router → Sink

A single message enters from an MQTT source, crosses two Wasm boundaries
(transform and router), and exits through a file sink. The diagram
highlights backpressure propagation via bounded `mpsc` channels.

```mermaid
sequenceDiagram
    participant Broker as MQTT Broker
    participant Src as Source Runner
    participant Q1 as mpsc (edge capacity)
    participant Tx as Transform Runner
    participant Q2 as mpsc (edge capacity)
    participant Rt as Router Runner
    participant Q3a as mpsc (port: alert)
    participant Sink as Sink Runner
    participant File as File System

    Broker->>Src: MQTT PUBLISH (QoS 1)
    activate Src
    Src->>Src: Wrap as RuntimeEnvelope (Arc header + Bytes payload)
    Src->>Q1: send(envelope)
    Note over Q1: Bounded channel; if full,<br/>OverflowPolicy applies<br/>(slow / drop / dead-letter)
    deactivate Src

    Q1->>Tx: recv(envelope)
    activate Tx
    Tx->>Tx: Inject borrow<buffer> into Store ResourceTable
    Tx->>Tx: Call guest transform.process(message)
    Tx-->>Tx: Guest returns Ok(output-message)
    Tx->>Tx: Build new RuntimeEnvelope from output bytes
    Tx->>Q2: send(new_envelope)
    deactivate Tx

    Q2->>Rt: recv(envelope)
    activate Rt
    Rt->>Rt: Inject borrow<buffer> into Store ResourceTable
    Rt->>Rt: Call guest router.route(message)
    Rt-->>Rt: Guest returns Ok(["alert"])
    Rt->>Q3a: send(envelope) to port "alert"
    deactivate Rt

    Q3a->>Sink: recv(envelope)
    activate Sink
    Sink->>File: Write payload bytes
    deactivate Sink
```

### Backpressure detail

The builder creates one `tokio::mpsc::channel` per destination receiver. Edge configuration remains edge-shaped: the physical queue uses the maximum explicit incoming capacity, or `engine.default_queue_capacity = 1024` only when no incoming edge specifies one. Zero capacities fail validation before channel construction. Each sender retains its own `OverflowPolicy`:

| Policy | Behaviour |
|--------|-----------|
| `slow` (default) | Reserve a permit and wait for space, propagating backpressure. |
| `drop` | On a full destination, discard without waiting and count the drop. |
| `dead-letter` | On a full destination, make a non-blocking send to the configured file or MQTT DLQ. |

A closed destination is distinct from overflow. Successful dead-letter delivery, DLQ-full, and DLQ-closed are separate counters. Source, Transform, and Filter broadcast to all downstream edges; Router selects labeled ports. Fan-out applies each matching edge's policy independently and attempts non-slow branches before waiting on slow branches.

Fan-in uses one destination receiver with cloned senders. No separate merge abstraction exists, and Tokio provides no fairness or cross-producer ordering guarantee.

---

## 2. Hot-Swap via Watch Channel

Hot-swap replaces a running Wasm node's `Store` and `Instance` with a
pre-compiled replacement **between messages**. The control plane sends the
new payload through a `watch::Sender<Option<SwapPayload>>`. Each runner
loop polls `swap_rx.has_changed()` at the top of every iteration —
**before** `select!`ing on cancellation, the swap signal and the input
channel — and applies the new payload before dequeuing the next message.
Because `swap_rx.changed()` is one of the `select!` branches, an idle runner
wakes for a swap immediately.

```mermaid
sequenceDiagram
    participant API as HTTP API (axum)
    participant Orch as Orchestrator
    participant Watch as watch::Sender<Option<SwapPayload>>
    participant Runner as Transform Runner (per-iteration has_changed poll)
    participant OldStore as Old Store + Instance
    participant NewStore as New Store + Instance (pre-instantiated)

    Note over API: POST /api/v1/nodes/{id}/hot-swap<br/>body: { wasm_path: "..." }
    API->>API: Read Wasm bytes from disk
    API->>API: prepare_transform_swap_timed:<br/>  1. Compile via ComponentCache + link (spawn_blocking)<br/>  2. Instantiate into a new Store
    API->>Orch: send_swap(node_id, SwapPayload)
    Orch->>Watch: watch_tx.send(Some(payload))

    Note over Runner: Runner loop iteration:<br/>  1. swap_rx.has_changed()? — apply if yes<br/>  2. select! { cancel | swap_rx.changed() | input_rx.recv() }

    Watch-->>Runner: swap_rx.changed() wakes the runner (or has_changed() on next iteration)
    activate Runner
    Runner->>Runner: Finish current message (if in-flight)
    Runner->>Runner: Flush retry buffer → DLQ (reason: HotSwapDrain)
    Runner->>OldStore: Drop old Store + Instance
    destroy OldStore
    Runner->>NewStore: Install, validate, and initialize replacement
    Runner->>Runner: Mark replacement adoption; resume main loop
    deactivate Runner

    Note over Runner: Next input_rx.recv() uses<br/>the new Store + Instance
```

### Key properties

- **Between-messages guarantee.** The runner never interrupts a guest call
  mid-execution. It observes the swap signal only at the top of the next
  loop iteration — after the current message completes (if any) and before
  the next one is dequeued from the `select!` branch.
- **Preparation off the message path.** Compilation and linking run on a blocking thread through the engine's component cache (the response reports `compile_cache` as `memory_hit`, `disk_hit`, or `compiled`), and typed instantiation happens before signalling. The runner installs the prepared values and runs guest validation/initialization before marking replacement adoption.
- **Stateless replacement.** The replacement starts from a fresh Store and its own `init`; guest state in the old instance is discarded, and the old guest's `close` export is not called.
- **Retry buffer flush.** Outstanding retry entries are sent to the DLQ
  with `DlqReason::HotSwapDrain` because the new plugin version may have
  incompatible semantics. No retries survive the boundary.
- **Serialized mutation.** Hot-swap and reconfigure share one per-node guard. Concurrent requests cannot overwrite a pending watch value: one proceeds and the other receives a conflict.
- **Runner-local reporting.** The response separates `replacement_adopted` from `first_post_replacement_local_outcome`. A local forward, filter drop, or no-route outcome is not sink convergence, sequence continuity, loss, or throughput evidence.
- **Rollback boundary.** Initialization rollback exists for every eligible Wasm role. Process-time canary rollback is implemented only for Transform; Filter and Router do not make that claim. A20 remains deferred, so `/metrics` has no rollback-total series.

For the full design rationale, see [ADR-0003](../adr/0003-hot-swap-mechanism.md)
and [ADR-0012](../adr/0012-watch-channel-hot-swap.md).

---

## 3. Error Dispatch Path

When a Wasm guest returns `Err(process-error)` — or the host traps the
instance — the runner classifies the error into one of five categories and
applies the configured policy.

### Classification

| WIT variant / Trap | `ErrorCategory` | Default action |
|--------------------|-----------------|----------------|
| `bad-input(msg)` | `BadInput` | → DLQ immediately |
| `dependency-failed(msg)` | `DependencyFailed` | → Retry (3×, 100 ms backoff, then DLQ) |
| `processing-failed(msg)` | `ProcessingFailed` | → Retry (2×, 100 ms backoff, then DLQ) |
| `timed-out` returned by the guest, epoch interruption or fuel exhaustion | `TimedOut` | → Skip (drop message, increment metric); after an epoch or fuel trap the Store is also replaced, while a guest-returned `timed-out` keeps its instance |
| `unrecoverable(msg)` or any other trap (panic, out-of-bounds access, memory limit) | `Unrecoverable` | → Drop the message (`dropped_on_recovery`) and re-instantiate from the cached `InstancePre` (`Recovering` state); not configurable |

The `timed_out` action applies to epoch and fuel traps as well as to the guest's own `timed-out` return. An epoch or fuel trap inside a Transform canary window is treated like any other trap there and triggers the process-time rollback.

### Retry buffer

Retryable errors (`dependency-failed`, `processing-failed`) enter a bounded `VecDeque` (default capacity 1000). The first retry waits exactly `backoff_ms`; later waits double to the 30-second cap. The runner selects the earliest due entry across the buffer and wakes for that deadline even when upstream input is idle.

`retry_count` is stored on `RuntimeEnvelope` and survives requeue and DLQ serialization. Once the retry budget is spent, the configured terminal action is honored: `skip` is counted as `retry_exhausted_skips`, `dlq` preserves `RetriesExhausted`, and `teardown` stops the node (see below). DLQ-full and DLQ-closed remain distinguishable and never requeue an exhausted envelope.

### `teardown` is a one-way stop

The configurable `teardown` action (for `bad_input`, `timed_out`, or a retry config's `exhausted`) counts the message as `dropped_on_teardown` and ends that node's runner loop. It does not enter `Recovering` and does not re-instantiate: the node stays stopped for the rest of the run, its input queue is no longer read, and upstream edges then see a closed destination. Its reported node state is not changed. Only the built-in `Unrecoverable` path below recovers.

### DLQ actions require a `[dead_letter]` sink

`bad_input`, `dependency_failed.exhausted`, and `processing_failed.exhausted` default to `dlq`, and the validator rejects a config whose processing nodes have an effective `dlq` action without a `[dead_letter]` section. Trapped and `unrecoverable` messages are written to the DLQ before recovery when a sink exists (`dlq_sent`); without one, which requires every action to be `skip` or `teardown`, they are counted as `dropped_on_recovery`. `dlq_lost` is left for a full or closed DLQ.

### DLQ envelope

Every message that reaches the dead-letter queue carries a `DlqEnvelope`
with full debugging context: `source_node`, `error_category`,
`error_message`, `retry_count`, the original `RuntimeEnvelope` (for
replay), and tracing correlation IDs (`trace_id`, `parent_id`).

### Recovery from `Unrecoverable`

When a guest returns `unrecoverable` or the host traps the call (other than an epoch or fuel trap handled by the `timed_out` action), the runner drops the message, enters `Recovering`, creates a fresh Store from the cached `InstancePre`, reapplies limits, and runs lifecycle validation and initialization. Success returns to `Running`; failure ends that node loop. Transform may first attempt its process-time canary rollback (once per swap) when one is active. Guest state does not survive recovery.

**Node state reporting.** Every `NodeStateTracker` starts in `Starting`. The Wasm Transform, Filter, and Router runner loops do not move a healthy node to `Running` after startup; only the recovery path above (and the native passthrough loop) does. A Wasm node that has never failed therefore reports `Starting` through `GET /api/v1/nodes` while it processes messages normally, and `Running` appears only after a recovery. Do not use `Running` as a startup-health signal in the current build.

### Shutdown (cancel-all)

Shutdown is cancel-all. On `POST /api/v1/pipeline/shutdown`, SIGINT, or
SIGTERM, one cancellation token fires for every node at once; sources are
not stopped first and messages still queued on edges are not drained
through the DAG. Each Wasm runner leaves its loop at the next input wait,
flushes its retry buffer to the DLQ with `DlqReason::Shutdown`, and drops
its `Store`; the guest `close()` export is not called. Sinks consume the
messages already buffered in their own queue, then run `flush()` and
`close()`. The orchestrator waits up to 5 s for all
node tasks and aborts any still running at that deadline, which fails the
run (exit status 3). A second SIGINT or SIGTERM exits immediately (130 / 143).

A finite pipeline (for example a `bench-source` with `total_messages`)
ends differently: the source exits, channels close in topological order,
and every node drains its queue before exiting.

---

## See also

- [ADR-0008 — Error Policy Engine](../adr/0008-error-policy-engine.md)
- [RFC-005 — Orchestrator](../rfcs/RFC-005-orchestrator.md) §D2 (runner loop)
- [RFC-002 — Host Runtime](../rfcs/RFC-002-host-runtime.md) §D5 (fuel/epoch metering)
