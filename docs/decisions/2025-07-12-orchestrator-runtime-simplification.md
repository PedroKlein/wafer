# Orchestrator & Runtime Simplification — Session 5 Decisions

**Date:** 2025-07-12  
**Status:** Decided (pending implementation)  
**Scope:** Phase 0 structural refactor — orchestrator, builder, runner loops, hot-swap coordinator  
**Depends on:** Sessions 1–4 (WIT contracts, host runtime, node type architecture, config schema)  
**Feeds into:** Implementation task graph, evaluation infrastructure  

---

## Context

With WIT contracts (Session 1), host runtime (Session 2), node type architecture (Session 3), and config schema (Session 4) decided, this session redesigns the orchestrator — the builder, queue wiring, runner loops, error policy executor, hot-swap coordinator, and shutdown sequence — to match the new architecture.

Key constraints from prior sessions that bound this design:
- `InstancePre<WaferState>` for fast instantiation (~5µs) (Session 2 D9)
- Persistent Store per node (Session 2 D10)
- Retry buffer: bounded `VecDeque<RetryEntry>` per node loop (Session 2 D4)
- Error policy engine maps `process-error` 5-category → `ErrorAction` (Session 2 D4)
- Merge = tokio mpsc multi-sender (Session 3 D9)
- No Joiner — removed entirely (Session 3 A1)
- Transform: strict 1:1, always produces output (Session 3 A2)
- Filter/Router: borrow-only, no DLQ pre-clone needed (Session 3 D11)
- Config is `BTreeMap<String, NodeDef>` with serde tag dispatch (Session 4 D1)
- Single `Config` type, no `DagConfig` (Session 4 D7)
- Edge uses `port` field (source port only) (Session 4 D6)
- Fuel resolution: `engine.fuel.{type}` default, per-node `fuel` override (Session 4 D5)

---

## Decision 1: Task-Per-Node Execution Model (Confirmed)

**Decision:** Each pipeline node runs as an independent `tokio::spawn` task. Communication between nodes is via bounded mpsc channels. There is NO central scheduling loop. Tokio's runtime IS the scheduler.

**Rationale:**
- DAG topology (fan-out, fan-in) requires parallelism across branches. A sequential scheduler (Torvyn's model) wastes 3 of 4 cores on a RPi 4 when a router fans out to 3 transforms.
- eKuiper uses the identical model (goroutine-per-operator + buffered channels). This is the established pattern for stream processors.
- Natural backpressure: bounded channel full → sender blocks → upstream slows. No demand-credit protocol needed.
- Per-node fault isolation: one task panic ≠ other tasks die.
- Per-node hot-swap: signal one task independently without stopping others.

**Measurement concern addressed:** The evaluation uses a native Rust baseline with the same Tokio runtime + same channels + no Wasm. The difference between native and WAFER = pure Wasm overhead. Tokio is a constant on both sides of the comparison, so it cancels out. Additionally, a single-flow baseline (no channels, inline calls) measures the architecture overhead itself (~200-500ns per hop — negligible vs ~10-50µs Wasm calls).

**Overhead on RPi 4 (4 cores):**
- Tokio task wake-up: ~100-200ns per message
- mpsc channel send/recv: ~50-100ns per message
- Wasm boundary crossing: ~10-50µs per message
- Architecture overhead is <2% of per-hop cost

---

## Decision 2: Builder Redesign — Map Iteration with Receiver-Keyed Queue Wiring

**Decision:** The builder iterates `BTreeMap<String, NodeDef>`, dispatches on the serde-tagged enum variant, and produces per-node bundles. Queue wiring uses a **receiver-keyed** map: one channel per unique `(to_node, to_port)` pair, with sender clones for merge edges.

**Rationale:** With map-keyed nodes (Session 4 D1), the builder is simple iteration. For merge (multiple edges to same downstream node), the key insight is: one receiver per downstream input, multiple sender clones sharing it. This is how tokio mpsc inherently works — it's multi-producer, single-consumer.

**Queue wiring algorithm:**
```rust
fn wire_queues(config: &Config) -> QueueWiring {
    // Group edges by destination (to_node, to_port)
    let mut edges_by_dest: HashMap<(String, String), Vec<&EdgeDef>> = HashMap::new();
    for edge in &config.edges {
        let to_port = "default".to_string(); // no Joiner, dest port always "default"
        edges_by_dest.entry((edge.to.clone(), to_port)).or_default().push(edge);
    }

    // Create ONE channel per unique destination, clone sender per source edge
    for ((to_node, to_port), edges) in &edges_by_dest {
        let capacity = edges.iter()
            .filter_map(|e| e.capacity)
            .max()
            .unwrap_or(config.engine.default_queue_capacity);
        
        let (sender, receiver) = tokio::sync::mpsc::channel(capacity);
        receivers.insert((to_node, to_port), receiver);

        for edge in edges {
            let from_port = edge.port.as_deref().unwrap_or("default");
            senders.push(EdgeSender {
                from_node: edge.from.clone(),
                from_port: from_port.to_string(),
                to_node: to_node.clone(),
                to_port: to_port.clone(),
                sender: sender.clone(), // merge = clone sender
                overflow: edge.overflow.unwrap_or_default(),
            });
        }
    }
}
```

**Merge is automatic:** Two edges pointing to the same `(to_node, "default")` share one channel with sender clones. Session 4 D3 validation ensures only Transform/Sink may have multiple inbound edges.

**Capacity conflict on merge:** Uses the maximum of all edges' capacities (most permissive).

---

## Decision 3: Three Per-Type Runner Loops

**Decision:** Three distinct per-type loop functions for Wasm nodes: `run_transform_loop`, `run_filter_loop`, `run_router_loop`. Source and Sink loops remain separate (unchanged in structure). No generic loop.

**Rationale:**
- Transform takes ownership, always produces output → branchless happy path
- Filter borrows, returns bool → no DLQ pre-clone, different downstream dispatch
- Router borrows, returns port list → fan-out logic unique to router
- A generic loop would need `match` on every message for behavior known at build time
- Monomorphized per-type loops eliminate per-message branching on the hot path

**Shared components:** All three loops share:
- `ErrorPolicyExecutor` (struct, owned per-loop)
- Retry buffer priority check (same logic)
- Cancel-safe select! pattern (recv only in select, Wasm call outside)
- Hot-swap watch channel check (between messages)
- Metrics recording (unconditional atomics)

---

## Decision 4: ErrorPolicyExecutor as Owned Struct

**Decision:** The `ErrorPolicyExecutor` is a struct owned by each node loop (not shared across nodes). It encapsulates the retry buffer, backoff calculation, and DLQ dispatch.

**Interface:**
```rust
struct ErrorPolicyExecutor {
    config: ResolvedErrorPolicy,     // pipeline defaults merged with per-node overrides
    retry_buffer: RetryBuffer,       // bounded VecDeque<RetryEntry>
    dlq_sender: Option<mpsc::Sender<DlqEnvelope>>,
}

struct RetryBuffer {
    entries: VecDeque<RetryEntry>,
    capacity: usize,  // from error_policy.retry_buffer_capacity, default 100
}

struct RetryEntry {
    envelope: RuntimeEnvelope,
    category: ErrorCategory,
    retry_count: u32,
    next_attempt_at: Instant,  // backoff_base * 2^retry_count, capped at 30s
}

impl ErrorPolicyExecutor {
    /// Map error to action and execute. Returns false if Teardown (caller must recover).
    fn handle(&mut self, error: WasmProcessError, envelope: RuntimeEnvelope) -> bool;
    
    /// Next retry whose backoff has expired. O(1) peek at front of VecDeque.
    fn next_ready_retry(&mut self) -> Option<RuntimeEnvelope>;
    
    /// Flush all pending retries to DLQ (shutdown/hot-swap).
    fn flush_to_dlq(&mut self, reason: &str);
    
    /// Pending retry count (for metrics).
    fn pending_retries(&self) -> usize;
}
```

**Error dispatch table:**

| WasmProcessError | ErrorAction | Detail |
|------------------|-------------|--------|
| `BadInput(msg)` | DLQ immediately | Data is wrong, retry won't help |
| `DependencyFailed(msg)` | Retry with backoff → DLQ on exhaustion | External resource down |
| `ProcessingFailed(msg)` | Retry N times → DLQ on exhaustion | Internal bug, maybe transient |
| `TimedOut` | Skip + log | Epoch interrupt fired |
| `Unrecoverable(msg)` | Teardown → Recovery state | Node must re-instantiate |

**Retry semantics:**
- Priority over fresh messages (check retry buffer first each iteration)
- Exponential backoff: `next_attempt_at = now + backoff_base * 2^retry_count`, capped at 30s
- If retry buffer full → new errors go directly to DLQ with reason "retry_buffer_full"
- Retries do NOT survive hot-swap — flushed to DLQ with reason "hot_swap_drain"
- Backoff prevents starvation of fresh messages (retries only ready at their scheduled time)

---

## Decision 5: InstancePre — Per-Node, Cached for Recovery

**Decision:** Each Wasm node holds its own `Arc<InstancePre<WaferState>>`. This is used for:
1. Recovery from `unrecoverable` errors (~5µs re-instantiation)
2. Config-only warm swaps (same plugin, new init)
3. Plugin hot-swap replaces it with a new `Arc<InstancePre>`

**No global shared cache for now.** Upgrading to a shared `HashMap<[u8; 32], Arc<InstancePre>>` keyed by content hash is trivial later but unnecessary for thesis scope (each plugin typically used by one node).

**Where InstancePre lives:**
- Built during builder phase (compile → pre-instantiate)
- Moved into the node task as part of its owned state
- Updated via watch channel during hot-swap (new InstancePre included in swap payload)

**Lifecycle:**
```
Build time:   load .wasm → compile (~8ms) → pre_instantiate (~100µs) → Arc<InstancePre>
              instantiate_from_pre (~5µs) → Store + Bindings (ready to call)

Hot-swap:     load new .wasm → compile → new Arc<InstancePre>
              send via watch channel → node instantiates from new pre

Recovery:     instantiate_from_pre (existing cached_pre) → fresh Store + Bindings (~5µs)
              call init() → resume processing

Config swap:  same InstancePre → instantiate → call init(new_config) → resume
```

---

## Decision 6: Hot-Swap via Watch Channel (Ownership Transfer)

**Decision:** Eliminate the double-lock pattern (`Mutex<HashMap<String, Arc<Mutex<AnyNode>>>>`). Each node task OWNS its node instance. Hot-swap signals are delivered via a `tokio::sync::watch` channel per Wasm node.

**Ownership model:**
- **Build time:** Orchestrator constructs nodes, builds queue wiring, resolves policies
- **Spawn time:** Each node + its state is MOVED into `tokio::spawn`. No shared references.
- **Hot-swap:** Orchestrator sends new instance via watch channel. Node checks between messages.
- **Orchestrator retains:** JoinHandles, watch senders, CancellationToken, Config, InstancePre cache

**Watch channel payload:**
```rust
type SwapSignal = watch::Receiver<Option<SwapPayload>>;

struct SwapPayload {
    new_store: Store<WaferState>,
    new_bindings: WasmBindings,
    new_pre: Arc<InstancePre<WaferState>>,
}
```

**Hot-swap flow (revised from ADR-0003):**
1. **PREPARE:** Orchestrator loads new .wasm → compiles → creates InstancePre → instantiates → validates → creates SwapPayload
2. **SIGNAL:** Orchestrator sends SwapPayload via `watch_tx.send(Some(payload))`
3. **FLIP:** Node loop (between messages) sees watch changed → flushes retry buffer to DLQ → replaces store/bindings/cached_pre → calls init() → resumes
4. **RETIRE:** Old Store dropped automatically when replaced (RAII)

**Drain semantics simplified:** There is no separate "drain" phase. The watch is checked between messages — so by definition, no Wasm call is in-flight when the swap happens. The node finishes its current message, checks the watch, swaps, continues. "Drain time" = time to finish current message (typically <1ms at 1000 msg/s).

**Consequences:**
- Zero mutex on the hot path
- No contention between processing and hot-swap
- No double-lock pattern
- Hot-swap latency = one message processing time (bounded by fuel/epoch)
- NodeStateTracker (atomics) remains readable from orchestrator for status queries

---

## Decision 7: Recovering State Machine

**Decision:** When `unrecoverable` fires, the loop transitions to `Recovering`, pauses receiving, re-instantiates from cached `InstancePre` (~5µs), calls `init()`, transitions back to `Running`, and resumes.

**State transitions:**
```
Running → Recovering → Running  (success)
Running → Recovering → Error    (re-instantiation or init failed — terminal)
```

**Implementation (inside loop):**
```rust
Err(WasmProcessError::Unrecoverable(msg)) => {
    state_tracker.set_state(NodeState::Recovering);
    policy.flush_to_dlq("unrecoverable");
    
    match engine.instantiate_from_pre(&cached_pre, capabilities).await {
        Ok((new_store, new_bindings)) => {
            store = new_store;
            bindings = new_bindings;
            if let Err(e) = bindings.call_init(&mut store, &node_config).await {
                state_tracker.set_state(NodeState::Error);
                break; // node is dead
            }
            state_tracker.set_state(NodeState::Running);
            continue; // resume from queue
        }
        Err(_) => {
            state_tracker.set_state(NodeState::Error);
            break; // node is dead
        }
    }
}
```

**Key invariants:**
- Messages in the queue are NOT lost — they remain in the mpsc channel
- Retry buffer is flushed (new instance may handle things differently)
- During Recovering, `state_tracker.is_drain_ready()` returns false (prevents hot-swap racing with recovery)
- Recovery cost: ~5µs + init() time. Invisible to upstream/downstream.

---

## Decision 8: DLQ Envelope Format

**Decision:** Structured DLQ envelope carrying full context for debugging, replay, and tracing.

```rust
#[derive(Debug, Clone, Serialize)]
pub struct DlqEnvelope {
    pub timestamp: u64,                    // when DLQ entry created
    pub source_node: String,               // node that produced the error
    pub error_category: ErrorCategory,     // from process-error 5-category
    pub error_message: String,             // human-readable
    pub retry_count: u32,                  // how many retries before DLQ (0 = never retried)
    pub reason: DlqReason,                 // why it ended up here
    pub original: OriginalMessage,         // full original for replay
    pub trace_id: Option<String>,          // lineage reconstruction
    pub parent_id: Option<String>,         // lineage reconstruction
}

#[derive(Debug, Clone, Serialize)]
pub struct OriginalMessage {
    pub id: String,
    pub source: String,
    pub content_type: String,
    pub metadata: Vec<(String, String)>,
    pub payload: Bytes,
}

#[derive(Debug, Clone, Serialize)]
pub enum DlqReason {
    BadInput,
    RetriesExhausted { max_retries: u32 },
    RetryBufferFull,
    HotSwapDrain,
    Shutdown,
    QueueFull { edge: String },
    RecoveryFailed,
}

#[derive(Debug, Clone, Copy, Serialize)]
pub enum ErrorCategory {
    BadInput,
    DependencyFailed,
    ProcessingFailed,
    TimedOut,
    Unrecoverable,
}
```

**Enables:**
- E-Swap-2 (message accounting): count by `original.id` + verify no duplicates
- RQ3b (zero loss): every message either reaches sink OR is in DLQ with documented reason
- Debugging: full context (which node, what error, how many retries, why DLQ)
- Replay: full original message preserved

---

## Decision 9: Config Diff — Plugin vs Config Changes

**Decision:** Config diff distinguishes three change types with different hot-swap strategies:

| Change Type | Detection | Action | Cost |
|-------------|-----------|--------|------|
| Plugin change | `old.plugin != new.plugin` | Full hot-swap (recompile, new InstancePre) | ~9ms |
| Config-only change | `old.config != new.config`, same plugin | Warm swap (same InstancePre, fresh instance, new init) | ~5µs |
| Both changed | Plugin AND config differ | Full hot-swap with new config | ~9ms |
| Error policy change | `old.error_policy != new.error_policy` | Include new resolved policy in swap payload | 0 (config only) |

**Diff logic:**
```rust
enum NodeChange {
    PluginChanged { new_plugin: String },
    ConfigChanged { new_config_json: String },
    BothChanged { new_plugin: String, new_config_json: String },
    PolicyChanged { new_policy: ResolvedErrorPolicy },
}

fn diff_node(old: &WasmNodeDef, new: &WasmNodeDef) -> Option<NodeChange> {
    let plugin_changed = old.plugin != new.plugin;
    let config_changed = old.config != new.config;
    match (plugin_changed, config_changed) {
        (true, _) => Some(NodeChange::PluginChanged { .. }),
        (false, true) => Some(NodeChange::ConfigChanged { .. }),
        (false, false) => None,
    }
}
```

**Structural changes (added/removed nodes, edge changes) require full pipeline restart.** This is explicitly out of thesis scope — eKuiper also restarts for rule structure changes.

---

## Decision 10: Unconditional Metrics

**Decision:** Atomic counters on every node are always-on. No `#[cfg(feature = "http-api")]` around counter increments. The HTTP endpoint (Prometheus scrape) remains feature-gated.

```rust
/// Always available, unconditional. One per node.
pub struct NodeMetrics {
    pub messages_processed: AtomicU64,
    pub messages_failed: AtomicU64,
    pub total_process_ns: AtomicU64,
    pub retries_attempted: AtomicU64,
    pub dlq_sent: AtomicU64,
    pub swap_count: AtomicU64,          // for E-Swap-3 time-series correlation
}

/// Feature-gated: exposition layer only
#[cfg(feature = "http-api")]
pub struct PrometheusExporter { /* reads from NodeMetrics atomics */ }
```

**Rationale:**
- AtomicU64 increment: ~5ns. At 10K msg/s = 50µs/s = 0.005% overhead.
- Always-on enables: test assertions, tracing context, hot-swap timing, RQ measurements without feature flags.
- Evaluation plan needs counters for all 15 experiments — feature-gating would break measurements.

---

## Decision 11: Graceful Shutdown Sequence

**Decision:** Ordered shutdown with guaranteed flush:

```
CancellationToken fires
  │
  ├─ 1. Sources stop polling (immediate — select! sees cancel)
  │
  ├─ 2. Processing nodes: select! sees cancel → stop recv
  │     Each node on exit:
  │       a. Flush retry buffer → DLQ (reason: "shutdown")
  │       b. Node dropped (close() called via RAII/Drop)
  │
  ├─ 3. Sinks: drain remaining messages from channel, call flush()
  │
  ├─ 4. DLQ task: drain its queue, flush sink, exit
  │     (with 5s timeout — don't hang forever)
  │
  └─ 5. Orchestrator: join all handles, set state Stopped
```

**Ordering guarantee:** Sources stop → channels drain naturally (bounded, FIFO) → retry buffers flush to DLQ → DLQ collects → DLQ sink closes.

**Critical invariant:** Retry buffer flush happens BEFORE DLQ channel closes (both happen inside the node task, which exits before the DLQ task is cancelled).

---

## Decision 12: Orchestrator Role — Setup, Watch, Teardown Only

**Decision:** The orchestrator's job is exclusively:
1. **Build:** Parse config → validate → build nodes → wire queues → resolve policies
2. **Spawn:** Create watch channels → bundle per task → tokio::spawn all
3. **Live ops:** Hot-swap (compile + send via watch), resync (diff configs), status queries
4. **Teardown:** Cancel token → join handles → report

The orchestrator does NOT hold references to node instances. After spawn, it retains only:
- `Vec<JoinHandle<()>>` — detect panics, await completion
- `HashMap<String, watch::Sender<Option<SwapPayload>>>` — signal hot-swaps
- `CancellationToken` — trigger shutdown
- `Config` — for diff on resync
- `Arc<NodeStateTracker>` per node — for status queries (atomic reads, no lock)
- `Arc<NodeMetrics>` per node — for metric reads

---

## Decision 13: Evaluation Baseline Stack

**Decision:** Three native baselines for overhead decomposition:

| Layer | What | Measures | Purpose |
|-------|------|----------|---------|
| Layer 0: single-flow | Tight loop, inline function calls, same MQTT I/O | Absolute performance floor | "Architecture overhead" = Layer 1 - Layer 0 |
| Layer 1: native-with-channels | Same Tokio tasks + mpsc, native Rust functions | Task-per-node overhead | "Isolation tax" = Layer 2 - Layer 1 |
| Layer 2: WAFER | Full system with Wasm boundaries | Total cost | "Competitive?" = Layer 2 vs eKuiper |

**Thesis presentation:** Stacked bar chart showing where each microsecond goes:
- Channel + task overhead: ~200-500ns (Layer 0 → 1)
- Wasm boundary + fuel + ResourceTable: ~10-50µs (Layer 1 → 2)
- This proves the architecture is efficient; the cost is inherent to isolation.

---

## Complete Node Loop Pseudocode

### Transform Loop
```rust
async fn run_transform_loop(
    mut transform: WasmTransform,       // owned, moved in
    mut receiver: Receiver<RuntimeEnvelope>,
    senders: Vec<EdgeSender>,
    mut swap_rx: watch::Receiver<Option<SwapPayload>>,
    mut policy: ErrorPolicyExecutor,
    cancel: CancellationToken,
    state: Arc<NodeStateTracker>,
    metrics: Arc<NodeMetrics>,
    mut cached_pre: Arc<InstancePre<WaferState>>,
) {
    loop {
        // 1. Hot-swap check (non-blocking, between messages)
        if swap_rx.has_changed().unwrap_or(false) {
            if let Some(payload) = swap_rx.borrow_and_update().take() {
                policy.flush_to_dlq("hot_swap_drain");
                transform.replace(payload.new_store, payload.new_bindings);
                cached_pre = payload.new_pre;
                continue;
            }
        }

        // 2. Retry buffer priority
        let envelope = if let Some(retry) = policy.next_ready_retry() {
            retry
        } else {
            // 3. Receive from channel (cancel-safe: only recv in select!)
            match select! {
                biased;
                () = cancel.cancelled() => None,
                msg = receiver.recv() => msg,
            } {
                Some(e) => e,
                None => break,
            }
        };

        // 4. DLQ safety clone (Arc + Bytes refcount = ~10ns)
        let safety = envelope.clone();

        // 5. Wasm call OUTSIDE select! (never cancelled)
        state.set_processing(true);
        let result = transform.process(envelope).await;
        state.set_processing(false);

        // 6. Dispatch
        match result {
            Ok(output) => {
                metrics.messages_processed.fetch_add(1, Relaxed);
                send_downstream(&senders, output).await;
            }
            Err(WasmProcessError::Unrecoverable(_)) => {
                metrics.messages_failed.fetch_add(1, Relaxed);
                if !recover(&mut transform, &cached_pre, &mut policy, &state).await {
                    break; // node is dead
                }
            }
            Err(e) => {
                metrics.messages_failed.fetch_add(1, Relaxed);
                policy.handle(e, safety);
            }
        }
    }
    policy.flush_to_dlq("shutdown");
}
```

### Filter Loop
```rust
async fn run_filter_loop(/* same signature pattern */) {
    loop {
        // 1. Hot-swap check (same as transform)
        // 2. Retry priority + recv (same as transform)
        let envelope = ...;

        // 3. No safety clone needed (we borrow)
        state.set_processing(true);
        let result = filter.evaluate(&envelope).await;
        state.set_processing(false);

        match result {
            Ok(FilterOutcome::Pass) => {
                metrics.messages_processed.fetch_add(1, Relaxed);
                send_downstream(&senders, envelope).await; // move original
            }
            Ok(FilterOutcome::Drop) => {
                metrics.messages_processed.fetch_add(1, Relaxed);
            }
            Err(WasmProcessError::Unrecoverable(_)) => {
                metrics.messages_failed.fetch_add(1, Relaxed);
                if !recover(&mut filter, &cached_pre, &mut policy, &state).await {
                    break;
                }
            }
            Err(e) => {
                metrics.messages_failed.fetch_add(1, Relaxed);
                policy.handle(e, envelope); // still own it
            }
        }
    }
    policy.flush_to_dlq("shutdown");
}
```

### Router Loop
```rust
async fn run_router_loop(/* same signature pattern */) {
    loop {
        // 1. Hot-swap check (same)
        // 2. Retry priority + recv (same)
        let envelope = ...;

        // 3. No safety clone needed (we borrow)
        state.set_processing(true);
        let result = router.route(&envelope).await;
        state.set_processing(false);

        match result {
            Ok(RouteOutcome::Ports(ports)) => {
                metrics.messages_processed.fetch_add(1, Relaxed);
                // Fan-out: clone for N-1, move for last (Session 3 D12)
                fan_out(&ports, envelope, &senders).await;
            }
            Ok(RouteOutcome::Drop) => {
                metrics.messages_processed.fetch_add(1, Relaxed);
            }
            Err(WasmProcessError::Unrecoverable(_)) => {
                metrics.messages_failed.fetch_add(1, Relaxed);
                if !recover(&mut router, &cached_pre, &mut policy, &state).await {
                    break;
                }
            }
            Err(e) => {
                metrics.messages_failed.fetch_add(1, Relaxed);
                policy.handle(e, envelope);
            }
        }
    }
    policy.flush_to_dlq("shutdown");
}
```

---

## Builder Sequence (Complete)

```
Config (parsed TOML, validated)
  │
  ├─ 1. Build DagGraph (petgraph, compute topo_order, validate acyclic/connected)
  │
  ├─ 2. Resolve error policies
  │     For each Wasm node: merge [error_policy] defaults + [nodes.X.error_policy] overrides
  │     Produce ResolvedErrorPolicy per node
  │
  ├─ 3. Resolve fuel
  │     For each Wasm node: per-node `fuel` field OR engine.fuel.{type} default
  │
  ├─ 4. Build nodes (in topo order)
  │     Source/Sink: construct from SourceDef/SinkDef (kind dispatch)
  │     Transform/Filter/Router:
  │       Load .wasm (local path or OCI pull)
  │       Compile → Component (~8ms)
  │       Pre-instantiate → Arc<InstancePre> (~100µs)
  │       Instantiate from pre → Store + Bindings (~5µs)
  │       Call validate() (fail-fast, no side effects)
  │
  ├─ 5. Wire queues (receiver-keyed)
  │     Group edges by destination (to_node, to_port)
  │     One mpsc channel per unique destination
  │     Clone sender per source edge (merge = multi-sender)
  │
  ├─ 6. Create control infrastructure
  │     CancellationToken (shared)
  │     watch::channel per Wasm node (for hot-swap)
  │     Arc<NodeStateTracker> per node
  │     Arc<NodeMetrics> per node
  │     DLQ channel + DLQ sink task
  │
  ├─ 7. Bundle per task
  │     (node, receivers, senders, error_policy, swap_rx, cancel, state_tracker, metrics, cached_pre)
  │
  └─ 8. Spawn (ordered)
        DLQ task first (so channel is ready for retry flushes)
        Node tasks (each owns its bundle — moved in)
        Store: JoinHandles + watch_tx handles in orchestrator
```

---

## Amendments to Prior Sessions

### Amendment to Session 2 (Host Runtime Architecture)

**Session 2 D4 (Error Policy):** The `ErrorPolicyConfig` schema is confirmed unchanged from Session 2/4. The new addition is the runtime `ErrorPolicyExecutor` struct that interprets it — this was left as "implementation detail" in Session 2 and is now specified.

### Amendment to ADR-0003 (Drain-and-Flip)

**Drain phase simplified:** With the watch channel design, there is no explicit "drain" phase where upstream is told to stop sending. Instead:
- The watch is checked between messages (the node is idle — no in-flight call)
- The swap happens atomically from the node's perspective
- "Drain time" = time to finish current message processing

This is simpler than ADR-0003's "stop routing → wait for in-flight → flip" because there's nothing to wait for — the check only happens when the node is already idle between messages.

**RoutingController (routing.rs) removed:** No longer needed. The current design buffers messages during drain, but with watch-channel swaps happening between messages, there's no window where messages need buffering.

### No amendments to Sessions 1, 3, or 4

All decisions from Sessions 1 (WIT contracts), 3 (node types), and 4 (config schema) remain unchanged. This session implements the runtime that realizes their designs.

---

## What This Removes (vs Current Implementation)

| Removed | Replacement |
|---------|-------------|
| `Mutex<HashMap<String, Arc<Mutex<AnyNode>>>>` | Ownership transfer + watch channel |
| `RunState` wrapped in `Mutex<Option<...>>` | Consumed once, destructured into bundles |
| `RoutingController` (routing.rs) | Watch channel swap between messages |
| `run_joiner_loop` | Deleted — merge is multi-sender |
| `DagConfig` struct | Deleted — `Config` only |
| `ProcessResult::Filter` in transform | Deleted — transform always emits (Session 3 A2) |
| `NodeAssembler` behind Mutex | Orchestrator compiles freely, sends result via watch |
| `from_dag_config()` builder | Deleted — takes `&Config` directly |
| `find_edge_overflow_policy()` | Overflow stored directly in EdgeSender |
| `collect_output_senders` / `collect_input_receivers` | Resolved during wire_queues, bundled per task |

---

## What This Adds

| Added | Purpose |
|-------|---------|
| `ErrorPolicyExecutor` struct | Encapsulates retry + backoff + DLQ per loop |
| `RetryBuffer` (bounded VecDeque) | Transient error recovery with backoff |
| `DlqEnvelope` struct | Structured dead-letter with full context |
| `watch::channel` per Wasm node | Hot-swap signaling without mutex |
| `NodeMetrics` (unconditional atomics) | Always-on counters for all experiments |
| `Recovering` state + re-instantiation | Graceful recovery from unrecoverable errors |
| Config diff: plugin vs config changes | Warm swap (5µs) vs full swap (9ms) |
| Layer 0 single-flow baseline | Overhead decomposition in evaluation |
| Ordered shutdown with retry flush | Zero message loss during teardown |

---

## Sources Consulted

| Source | What it informed |
|--------|-----------------|
| Torvyn `torvyn-reactor/src/coordinator.rs` | D1 (task-per-flow vs task-per-node trade-offs) |
| Torvyn `torvyn-reactor/src/flow_driver.rs` | D3 (sequential scheduling loop, error policy enum) |
| Torvyn `torvyn-pipeline/src/builder.rs` | D2 (builder validation pattern, edge resolution) |
| Tremor-rs (findings §7.2) | D1 (contraflow = similar to bounded channel backpressure) |
| eKuiper (findings §9.1) | D1 (goroutine-per-operator validates task-per-node) |
| Wassette (findings §8.2) | D5 (Arc<InstancePre> pattern at enterprise scale) |
| Flow-Like (findings §2.4) | D5 (AOT cache key design: blake3 + platform) |
| Fluvio SmartModules (findings §4.3) | D6 (shared Engine, per-request Store lifecycle) |
| Tokio docs: Graceful Shutdown | D11 (cancel → drain → close pattern) |
| Web: task-supervisor patterns | D7 (classify failure → restart/escalate policy) |
| Web: bounded retry queue patterns | D4 (VecDeque + exponential backoff) |
| Thesis statement v3 (RQ1-3) | D10, D13 (evaluation needs unconditional metrics + baselines) |
| Evaluation plan (15 experiments) | D8, D10, D13 (DLQ format for message accounting) |
| Counter-arguments S1-S3 | D4, D6 (retry addresses "just restart" critique; minimal overhead) |
| Lee & Parks "Dataflow Process Networks" (1995) | D1 (actors with blocking reads on bounded FIFOs) |
| Erlang/OTP supervisor trees | D4, D7 (classify → restart/escalate/ignore) |
| Hoffmann/Megaphone (2019) | D6 (quiescence point = between messages) |
| Flink blue-green (2024) | D6 (prepare/flip lifecycle validated) |
| ADR-0003 drain-and-flip | D6 (simplified — no separate drain phase needed) |
| ADR-0002 bounded queues | D2 (mpsc channels confirmed, merge = multi-sender) |
