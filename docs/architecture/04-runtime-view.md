# 04 — Runtime View

> arc42 §6 — Runtime scenarios that illustrate the system's dynamic behaviour.

This section shows how key runtime flows execute inside WAFER. Each scenario
is presented as a sequence diagram. For the static structure these flows
traverse, see [03-building-blocks.md](./03-building-blocks.md).

> **⚠ Documentation drift.** Some scenarios below describe behavior that is
> only partially implemented — specifically the hot-swap phase telemetry
> (gap **A3**), the ACK-phase `init()` call (gap **A4**), production Wasm
> lifecycle `validate()` / `init()` calls (gap [**A14**](../status/implementation-gaps.md#a14)), lineage assignment
> for `trace_id` / `parent_id` (gap [**A13**](../status/implementation-gaps.md#a13)), the generic per-node-type
> dispatch on `/hot-swap` (gap **A10**), the retry-exhaustion + `Recovering`
> state transitions (gap **A7**), capability-aware instantiation (gap
> **A9**), and benchmark-backed hot-swap/RQ results that still use stub
> `TransformInstance` benches (gap [**A15**](../status/implementation-gaps.md#a15)). See
> [`../status/implementation-gaps.md`](../status/implementation-gaps.md) for
> the full catalogue and current code pointers.

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

Every edge is a `tokio::mpsc::channel` whose capacity comes from the
per-edge `capacity` field (defaults to `engine.default_queue_capacity = 1024`).
When a sender calls `send()` and the channel is full, the configured
`OverflowPolicy` applies:

| Policy | Behaviour |
|--------|-----------|
| `slow` (default) | Sender `await`s until space frees — backpressure propagates upstream. |
| `drop` | Message is silently dropped; sender continues immediately. |
| `dead-letter` | Message is routed to the DLQ; sender continues. |

Fan-in (multiple upstream edges converging on one node) uses the same
`mpsc` receiver — multiple senders, single consumer. No separate merge
abstraction exists; the receiver drains from all producers fairly.

---

## 2. Hot-Swap via Watch Channel

Hot-swap replaces a running Wasm node's `Store` and `Instance` with a
pre-compiled replacement **between messages**. The control plane sends the
new payload through a `watch::Sender<Option<SwapPayload>>`. Each runner
loop polls `swap_rx.has_changed()` at the top of every iteration —
**before** `select!`ing on cancellation vs the input channel — and applies
the new payload before dequeuing the next message.

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
    API->>API: prepare_transform_swap_timed:<br/>  1. Compile component (AOT)<br/>  2. Pre-instantiate (InstancePre)
    API->>Orch: send_swap(node_id, SwapPayload)
    Orch->>Watch: watch_tx.send(Some(payload))

    Note over Runner: Runner loop iteration:<br/>  1. swap_rx.has_changed()? — apply if yes<br/>  2. select! { cancel | input_rx.recv() }

    Watch-->>Runner: swap_rx.has_changed() returns true on next iteration
    activate Runner
    Runner->>Runner: Finish current message (if in-flight)
    Runner->>Runner: Flush retry buffer → DLQ (reason: HotSwapDrain)
    Runner->>OldStore: Drop old Store + Instance
    destroy OldStore
    Runner->>NewStore: Fence in pre-instantiated replacement
    Runner->>Runner: Resume main loop (select! on input/cancel)
    deactivate Runner

    Note over Runner: Next input_rx.recv() uses<br/>the new Store + Instance
```

### Key properties

- **Between-messages guarantee.** The runner never interrupts a guest call
  mid-execution. It observes the swap signal only at the top of the next
  loop iteration — after the current message completes (if any) and before
  the next one is dequeued from the `select!` branch.
- **Pre-instantiation off the hot path.** Compilation and `InstancePre`
  creation happen in the API handler (or a background task). The runner
  only performs the final Store/Instance swap — a sub-microsecond pointer
  exchange.
- **Retry buffer flush.** Outstanding retry entries are sent to the DLQ
  with `DlqReason::HotSwapDrain` because the new plugin version may have
  incompatible semantics. No retries survive the boundary.
- **`watch` channel semantics.** `tokio::sync::watch` is a single-producer
  single-consumer latest-value channel. If a second swap arrives before the
  runner processes the first, only the latest payload is observed — which
  is the correct behaviour (the most recent binary wins).
- **Timeline recording.** `SwapTimeline` captures `compile_ns` and
  `instantiate_ns` during preparation; `signal_ns`, `ack_ns`, and
  `convergence_ns` are recorded internally for benchmarking
  (see `docs/benchmarks/hot-swap.md`).

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
| `timed-out` (epoch/fuel interrupt) | `TimedOut` | → Skip (drop message, increment metric) |
| `unrecoverable(msg)` or host trap (panic, OOM, capability violation) | `Unrecoverable` | → Teardown node → `Recovering` state |

### Retry buffer

Retryable errors (`dependency-failed`, `processing-failed`) push the
`RuntimeEnvelope` into a bounded `VecDeque` (capacity default: 100) with
exponential backoff (`backoff_ms × 2^retry_count`, capped at 30 s). The
runner polls the buffer between input messages.

**Current status:** the retry buffer plumbing exists
(`crates/wafer-core/src/runner/error_policy.rs`), but retry-count
increment and `DlqReason::RetriesExhausted` emission are not yet wired
in `try_retry` — retryable errors currently re-enter the buffer with
`retry_count = 0` until the buffer overflows and the oldest entry is
evicted to the DLQ with `DlqReason::RetryBufferFull`. Full
retries-exhausted semantics are planned; see `docs/rfcs/RFC-002-host-runtime.md`.

### DLQ envelope

Every message that reaches the dead-letter queue carries a `DlqEnvelope`
with full debugging context: `source_node`, `error_category`,
`error_message`, `retry_count`, the original `RuntimeEnvelope` (for
replay), and tracing correlation IDs (`trace_id`, `parent_id`).

### Recovery from `Unrecoverable`

**Target design:** When a node enters `Recovering`, the runner
re-instantiates from the cached `InstancePre`. On success the node
returns to `Running`; on failure the retry buffer is flushed to DLQ
with `DlqReason::RecoveryFailed` and the failure is reported to the
orchestrator for escalation.

**Current status:** recovery is not yet implemented. All three runner
loops (`runner/transform.rs`, `runner/filter.rs`, `runner/router.rs`)
currently log the unrecoverable error and `break` — the node exits
rather than transitioning through `Recovering`. See
`docs/rfcs/RFC-002-host-runtime.md` for the target lifecycle.

### Graceful shutdown

On `POST /api/v1/pipeline/shutdown` or SIGINT, every runner flushes its
retry buffer to the DLQ with `DlqReason::Shutdown`, then drops its
`Store` and exits. Sources stop producing first so the pipeline drains
naturally through the DAG before Wasm nodes shut down.

---

## See also

- [ADR-0008 — Error Policy Engine](../adr/0008-error-policy-engine.md)
- [RFC-005 — Orchestrator](../rfcs/RFC-005-orchestrator.md) §D2 (runner loop)
- [RFC-002 — Host Runtime](../rfcs/RFC-002-host-runtime.md) §D5 (fuel/epoch metering)
