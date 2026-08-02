# RFC-005: Orchestrator & Runtime Simplification

- **Status:** Implemented — orchestrator, hot-swap telemetry, init-on-flip, warm swap, config cascade, lineage assignment, and Wasm lifecycle calls are all live in production (see closed gaps A3, A4, A5, A6, A10, A13, A14, and A15 in the ledger, all landed 2026-07-19 through 2026-07-21). Residual gap **A17** (process-time hot-swap rollback) is tracked in [`docs/status/implementation-gaps.md`](../status/implementation-gaps.md).
- **Original session date:** 2026-07-12
- **Amends:** ADR-0003 (drain-and-flip → watch-channel between-messages)

> **Implementation notes.** The watch-channel primitive is live and
> hot-swap works end-to-end (see
> `crates/wafer-core/tests/pipeline_e2e.rs::test_hot_swap_uppercase_to_passthrough`).
> Five-phase `SwapTimeline` (`compile` / `instantiate` / `signal` / `ack`
> / `convergence`) is reported through the API (A3 + A3b). ACK-phase
> `init()` is called on the new instance (A4). Production Wasm nodes
> run guest `validate()` and `init()` before the first message (A14).
> Config-only warm swap via `cached_pre()` is served by
> `POST /api/v1/nodes/{id}/reconfigure` (A5). The API dispatches per
> node type: transform/filter/router (A10). The builder resolves the
> per-pipeline + per-node `[error_policy]` cascade (A6). DLQ lineage is
> assigned at source ingress and preserved through fan-out (A13).

## Abstract

This RFC redesigns the WAFER orchestrator — builder, queue wiring, runner loops, error policy executor, hot-swap coordinator, and shutdown sequence — to align with the new WIT contracts (RFC-001), host runtime (RFC-002), node type architecture (RFC-003), and config schema (RFC-004). The central change replaces the double-lock `Mutex<HashMap<…>>` ownership model and the explicit drain-and-flip hot-swap (ADR-0003) with a task-per-node execution model where each node **owns** its instance, and hot-swap signals arrive via a per-node `tokio::sync::watch` channel checked between messages. This eliminates all mutexes from the hot path, simplifies "drain time" to zero (no in-flight call when the swap fires), and enables unconditional atomic metrics, structured DLQ envelopes, and a three-layer native baseline stack for evaluation overhead decomposition.

## Context

With WIT contracts (Session 1), host runtime (Session 2), node type architecture (Session 3), and config schema (Session 4) decided, this session redesigns the orchestrator to match the new architecture.

Key constraints from prior sessions that bound this design:
- `InstancePre<WaferState>` for fast instantiation (~5µs) (RFC-002 D9)
- Persistent Store per node (RFC-002 D10)
- Retry buffer: bounded `VecDeque<RetryEntry>` per node loop (RFC-002 D4)
- Error policy engine maps `process-error` 5-category → `ErrorAction` (RFC-002 D4)
- Merge = tokio mpsc multi-sender; no Joiner node (RFC-003 A1, D9)
- Transform: strict 1:1, always produces output (RFC-003 A2)
- Filter/Router: borrow-only, no DLQ pre-clone needed (RFC-003 D11)
- Config is `BTreeMap<String, NodeDef>` with serde tag dispatch (RFC-004 D1)
- Edge uses a single `port` field (source port only) (RFC-004 D6)
- Fuel resolution: `engine.fuel.{type}` default, per-node `fuel` override (RFC-004 D5)

## Decisions

### Decision 1: Task-Per-Node Execution Model (Confirmed)

Each pipeline node runs as an independent `tokio::spawn` task. Communication between nodes is via bounded mpsc channels. There is no central scheduling loop — Tokio's runtime IS the scheduler.

Rationale: DAG topology requires parallelism across branches; a sequential scheduler wastes cores when a router fans out. eKuiper uses the identical model (goroutine-per-operator + buffered channels). Natural backpressure: bounded channel full → sender blocks → upstream slows. Per-node fault isolation and per-node hot-swap follow naturally.

Architecture overhead on RPi 4: Tokio task wake-up ~100-200ns, mpsc send/recv ~50-100ns — both negligible vs ~10-50µs Wasm boundary crossing (<2% of per-hop cost).

### Decision 2: Builder Redesign — Receiver-Keyed Queue Wiring

The builder iterates `BTreeMap<String, NodeDef>`, dispatches on the serde-tagged enum variant, and produces per-node bundles. Queue wiring uses a **receiver-keyed** map: one channel per destination node (each node has a single default input port), with sender clones for merge edges.

Merge is automatic: two edges pointing to the same destination share one channel with sender clones. Capacity conflict on merge uses the maximum of all edges' capacities (most permissive).

### Decision 3: Three Per-Type Runner Loops

Three distinct per-type loop functions: `run_transform_loop`, `run_filter_loop`, `run_router_loop`. No generic loop — monomorphized per-type loops eliminate per-message branching on the hot path.

Shared components: `ErrorPolicyExecutor`, retry buffer priority check, cancel-safe `select!` pattern, hot-swap watch channel check (between messages), unconditional atomic metrics.

### Decision 4: ErrorPolicyExecutor as Owned Struct

The `ErrorPolicyExecutor` is a struct owned by each node loop (not shared across nodes). It encapsulates the retry buffer (bounded `VecDeque<RetryEntry>`), backoff calculation, and DLQ dispatch.

Error dispatch: `BadInput` → DLQ immediately; `DependencyFailed` → retry with backoff → DLQ on exhaustion; `ProcessingFailed` → retry N times → DLQ; `TimedOut` → skip + log; `Unrecoverable` → teardown + recovery.

Retry semantics: priority over fresh messages; exponential backoff capped at 30s; retries do NOT survive hot-swap (flushed to DLQ with reason `hot_swap_drain`).

### Decision 5: InstancePre — Per-Node, Cached for Recovery

Each Wasm node holds its own `Arc<InstancePre<WaferState>>`, used for: recovery from `unrecoverable` errors (~5µs re-instantiation), config-only warm swaps, and plugin hot-swap replaces it with a new `Arc<InstancePre>`.

Lifecycle: build-time compile (~8ms) → pre-instantiate (~100µs) → instantiate from pre (~5µs) → Store + Bindings ready. Hot-swap sends a new InstancePre via watch channel.

### Decision 6: Hot-Swap via Watch Channel (Ownership Transfer)

Eliminate the double-lock pattern. Each node task OWNS its node instance. Hot-swap signals are delivered via a `tokio::sync::watch` channel per Wasm node.

Watch channel payload: `SwapPayload` containing new Store, new bindings, and new `Arc<InstancePre>`.

Hot-swap flow:
1. **PREPARE:** Orchestrator loads new .wasm → compiles → creates InstancePre → instantiates → validates → creates SwapPayload
2. **SIGNAL:** Orchestrator sends SwapPayload via `watch_tx.send(Some(payload))`
3. **FLIP:** Node loop (between messages) sees watch changed → flushes retry buffer to DLQ → replaces store/bindings/cached_pre → calls init() → resumes
4. **RETIRE:** Old Store dropped automatically when replaced (RAII)

Drain semantics simplified: there is no separate "drain" phase. The watch is checked between messages — no Wasm call is in-flight when the swap happens. "Drain time" = time to finish current message (typically <1ms at 1000 msg/s).

**This decision amends ADR-0003** — the original "stop routing → wait for in-flight → flip" is replaced by the watch-channel between-messages model. The `RoutingController` (routing.rs) is removed entirely.

### Decision 7: Recovering State Machine

When `unrecoverable` fires, the loop transitions to `Recovering`, pauses receiving, re-instantiates from cached `InstancePre` (~5µs), calls `init()`, transitions back to `Running`, and resumes. Messages in the queue are NOT lost; the retry buffer is flushed; during Recovering, `is_drain_ready()` returns false.

### Decision 8: DLQ Envelope Format

Structured `DlqEnvelope` carrying: timestamp, source node, error category, error message, retry count, reason (enum: `BadInput`, `RetriesExhausted`, `RetryBufferFull`, `HotSwapDrain`, `Shutdown`, `QueueFull`, `RecoveryFailed`), full original message for replay, trace_id, parent_id.

Enables E-Swap-2 message accounting, RQ3b zero-loss verification, debugging, and replay.

### Decision 9: Config Diff — Plugin vs Config Changes

Config diff distinguishes: plugin change (full hot-swap, ~9ms), config-only change (warm swap via same InstancePre + fresh init, ~5µs), both changed (full swap), error policy change (config only, 0 cost). Structural changes (added/removed nodes, edge changes) require full pipeline restart.

### Decision 10: Unconditional Metrics

Atomic counters (`NodeMetrics`) on every node are always-on (~5ns per increment). The HTTP Prometheus endpoint remains feature-gated. This ensures all 15 evaluation experiments can gather measurements without special builds.

### Decision 11: Graceful Shutdown Sequence

Ordered shutdown: (1) Sources stop polling via CancellationToken, (2) Processing nodes stop recv, flush retry buffers to DLQ, close() via Drop, (3) Sinks drain remaining messages, flush(), (4) DLQ task drains queue with 5s timeout, (5) Orchestrator joins all handles.

### Decision 12: Orchestrator Role — Setup, Watch, Teardown Only

After spawn, the orchestrator retains only: `JoinHandle`s, `watch::Sender`s, `CancellationToken`, `Config` (for diff), `Arc<NodeStateTracker>` per node (atomic reads), `Arc<NodeMetrics>` per node.

### Decision 13: Evaluation Baseline Stack

Three native baselines: Layer 0 (single-flow, inline calls — floor), Layer 1 (native-with-channels — task-per-node overhead), Layer 2 (WAFER — full Wasm boundaries). Stacked bar chart decomposes where each microsecond goes.

## Alternatives Considered

**Central scheduler loop (Torvyn's model):** Rejected because it wastes cores on multi-branch DAGs. On RPi 4 with 4 cores, a sequential scheduler leaves 3 cores idle when a router fans out to 3 transforms.

**Generic runner loop with per-message match:** Rejected because it introduces per-message branching on the hot path for behavior known at build time. Monomorphized per-type loops are faster.

**Shared InstancePre global cache (hash-keyed):** Rejected for thesis scope — each plugin is typically used by one node. Trivial to upgrade later.

**Mutex-based hot-swap (existing implementation):** Rejected — the double-lock `Mutex<HashMap<String, Arc<Mutex<AnyNode>>>>` pattern introduces contention between processing and hot-swap, adds per-message locking overhead, and complicates the shutdown sequence.

**Explicit drain phase (ADR-0003 original):** Rejected — with watch-channel checked between messages, no in-flight call exists when the swap fires, making an explicit drain phase unnecessary and removing the `RoutingController` entirely.

**Feature-gated metrics:** Rejected — AtomicU64 increment at ~5ns is negligible overhead; always-on enables test assertions, tracing context, and all 15 evaluation experiments without special builds.

## Related RFCs

- **RFC-001** — provides the WIT-typed boundaries that this RFC's runner loops call into.
- **RFC-002** — defines the host runtime (InstancePre lifecycle, persistent Store, error policy categories) that this RFC orchestrates.
- **RFC-003** — defines the three Wasm node types (Transform, Filter, Router) and eliminates the Joiner, producing the three loop variants in D3. Also defines merge as multi-producer mpsc (D9) that this RFC's builder wires.
- **RFC-004** — defines the config schema (`BTreeMap<String, NodeDef>`, `port` field, fuel resolution) that this RFC's builder consumes.
- **ADR-0003** — the original drain-and-flip hot-swap mechanism. **This RFC amends ADR-0003**: the 4-phase "stop routing → drain in-flight → flip → resume" is replaced by the watch-channel between-messages model (D6) which eliminates the drain phase entirely.

## Implementation Notes

- The watch-channel hot-swap model (D6) is fully implemented in `crates/wafer-core/src/orchestrator/hotswap.rs`, which provides `prepare_transform_swap`, `prepare_filter_swap`, and `prepare_router_swap` functions that package a compiled component into a `SwapPayload` ready to send via the watch channel.
- `crates/wafer-core/src/orchestrator/builder.rs` creates a `watch::Sender<Option<SwapPayload>>` per Wasm node (stored in `watch_senders: HashMap<Box<str>, watch::Sender<…>>`), and threads the corresponding `watch::Receiver` into each node's runner bundle (via the `swap_rx` field on `TransformBundle`, `FilterBundle`, and `RouterBundle`).
- The orchestrator retains `watch_senders` after spawn for signaling hot-swaps — matching D12's design.
- The three per-type runner loops (D3) are implemented in `crates/wafer-core/src/runner/` with the shared `select!` + watch-check pattern described in D6.
- ADR-0003's `RoutingController` is absent from the codebase; the watch-channel model fully replaces it.
- Code matches decisions; no divergence found.
