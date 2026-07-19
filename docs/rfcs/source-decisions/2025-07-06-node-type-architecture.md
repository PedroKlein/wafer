# Node Type Architecture — Session 3 Decisions

**Date:** 2025-07-06  
**Status:** Decided (pending implementation)  
**Scope:** Phase 0 structural refactor — host-side node type system  
**Depends on:** `2025-07-05-wit-contracts-envelope-design.md` (Session 1), `2025-07-06-host-runtime-architecture.md` (Session 2)  
**Amends:** Session 1 (removes Joiner world, simplifies Transform return type), Session 2 (redesigns RuntimeEnvelope with Arc header)  

---

## Context

With WIT contracts (Session 1) and host runtime architecture (Session 2) decided, this session designs the node type system — the Rust types, traits, and dispatch patterns that realize those contracts. Key inputs:

- **4 WIT worlds from Session 1**: transform-node, filter-node, router-node, joiner-node
- **Task-per-node architecture** with bounded mpsc channels
- **borrow<buffer> input**, `list<u8>` output (asymmetric payload crossing)
- **5-category process-error** with per-node error policy (Session 2 D4)
- **InstancePre** for hot-swap fast instantiation (Session 2 D9)
- **Persistent Store per node** (Session 2 D10)

This session also revisits the operator taxonomy from first principles, producing **amendments to Sessions 1 and 2**.

---

## Amendment A1: Remove Joiner WIT World

**Decision:** Remove the `joiner-node` world and `joiner` interface entirely. Merge is a host-native topology operation — not a Wasm plugin.

**Rationale (from comprehensive research):**

The stream processing literature distinguishes two fundamentally different N→1 operations:

| | **Merge** | **Join** |
|---|-----------|---------|
| What it does | Interleaves N streams into 1 | Correlates records, produces derived output |
| Logic needed | NONE — pure topology | YES — correlation key, time window, combine function |
| Creates new records? | No — preserves upstream records unchanged | Yes — new derived records |
| Stateful? | No | Yes (buffers records waiting for match) |

WAFER's "Joiner" was trying to be both. But:
- **Merge** needs no Wasm — it's just tokio mpsc (multiple producers, single consumer). Multiple upstream nodes send to the same receiver channel. Zero cost.
- **Join** requires windowed state — explicitly out of thesis scope (Session 1: "Windowed joins documented as out-of-scope limitation").

**Evidence from comparators:**
- **Kafka Streams**: `merge()` (no logic, just interleave) is completely separate from `join()` (correlate + derive)
- **Azure IoT**: `concatenate` operator needs no Wasm module — it's built-in topology
- **Flink**: Union is a topology operation, not a user-defined operator

**Impact:** `pipeline:routing@0.1.0` loses the `joiner` interface and `joiner-node` world. 3 WIT worlds remain: `transform-node`, `filter-node`, `router-node`.

---

## Amendment A2: Transform Is Strict 1:1

**Decision:** Transform MUST always return an `output-message` or an error. Remove the `filter` variant from `process-outcome`. The WIT return type simplifies to `result<output-message, process-error>`.

**Rationale:**

- **Flink's `MapFunction`** is strict 1:1: "A Map function always produces a single result element for each input element." Most successful stream processor's explicit design.
- **Filter exists for dropping.** With Filter as a first-class node type, Transform no longer needs the degenerate "I chose not to emit" path.
- **Plugin DX is clearer:** Transform transforms. Filter filters. No ambiguity about which to use.
- **Host optimization:** The loop knows output is always coming → branchless happy path, no "did it skip?" check.
- **If a component can't process input, it returns an error** (`bad-input` category → DLQ). Pipeline designers place Filter upstream for validation.

**Updated WIT:**
```wit
interface transform {
    use pipeline:types/types.{message, output-message, process-error};
    process: func(input: message) -> result<output-message, process-error>;
}
```

**Impact:** `process-outcome` variant is removed from `pipeline:types`. Transform return is now `result<output-message, process-error>` directly.

---

## Amendment A3: RuntimeEnvelope Redesigned with Arc Header

**Decision:** Split RuntimeEnvelope into immutable header (Arc-wrapped), payload (Bytes), and mutable lineage. This makes cloning nearly free for borrow-only nodes.

**Rationale:**

Filter and Router borrow the envelope — they never modify it. When they pass through or fan-out, the host must "clone" the envelope for each downstream queue. With the old design, this clones every string field (~200-400 bytes of allocations per clone). With Arc:

- Filter pass-through: Arc refcount bump + Bytes refcount bump = **~16 bytes** (vs ~300B before)
- Router fan-out to 3 ports: 3 refcount bumps = **0 bytes allocated** (vs ~900B before)
- Transform DLQ safety copy: Arc refcount bump = **near-free** (vs full clone before)

**Updated RuntimeEnvelope:**
```rust
#[derive(Debug, Clone)]
pub struct RuntimeEnvelope {
    /// Immutable message identity — shared via refcount across fan-out.
    pub header: Arc<EnvelopeHeader>,
    /// Payload bytes — refcounted, zero-copy from MQTT source.
    pub payload: Bytes,
    /// Host-private lineage (mutable per-hop).
    pub(crate) lineage: Lineage,
}

/// Immutable fields. Created once at source/transform output. Never modified.
#[derive(Debug)]
pub struct EnvelopeHeader {
    pub id: String,
    pub timestamp: u64,
    pub source: String,
    pub content_type: String,
    pub metadata: Vec<(String, String)>,
}

/// Mutable per-hop tracking. Small enough to copy.
#[derive(Debug, Clone, Default)]
pub(crate) struct Lineage {
    pub parent_id: Option<String>,
    pub trace_id: Option<String>,
}
```

**Clone cost:** Arc increment (~5ns) + Bytes increment (~5ns) + Lineage copy (~32B stack copy) = **~10ns total**. Compared to old: 5+ String heap allocations = ~100-200ns.

---

## Decision 1: Node Type Set — 4 Types in Two Categories

**Decision:** WAFER has exactly 4 node types organized into two categories:

### Category 1: Data Transformation (ownership, produces new bytes)
- **Transform** (Wasm): Strict 1:1. Takes message, MUST return new output-message.

### Category 2: Flow Control (borrow-only, zero-copy possible)
- **Filter** (Wasm): Pure predicate. Borrows message, returns bool.
- **Router** (Wasm): Routing decision. Borrows message, returns port list.
- **Merge** (Host-native): Topology operation. Multiple inputs → one output. No Wasm.

### Not included (documented as future work):
- **Aggregate/Window**: Requires stateful accumulation over time windows.
- **Join**: Requires windowed state for correlation.
- **FlatMap/Splitter**: 1→N output breaks backpressure guarantees.
- **Enrich**: 1:1 with side-lookup from state store — needs host-provided kv-store import.
- **Dedup**: Idempotent receiver — needs state (seen-message set).

All of the above can be added as future WIT interfaces without modifying the runtime architecture (task-per-node, bounded queues, error policy, hot-swap all support them).

---

## Decision 2: AnyNode Structure — Struct with Inner Enum

**Decision:** Extract state tracker and node identity into a shared wrapper struct. The polymorphic behavior lives in `NodeKind`.

```rust
pub struct AnyNode {
    pub(crate) id: String,
    pub(crate) state_tracker: Arc<NodeStateTracker>,
    pub(crate) kind: NodeKind,
}

pub enum NodeKind {
    Source(Box<dyn Source>),
    Sink(Box<dyn Sink>),
    Transform(Box<dyn Transform>),
    Filter(Box<dyn Filter>),
    Router(Box<dyn Router>),
    // No Merge variant — Merge is DAG wiring, not a node.
}
```

**Rationale:** Eliminates repetitive `Arc<NodeStateTracker>` in every variant. Adding new types doesn't require touching every pattern match for state access. `node.state_tracker` works uniformly.

---

## Decision 3: Keep Trait Objects for Processing Nodes

**Decision:** Keep `dyn Transform`, `dyn Filter`, `dyn Router` — do NOT switch to concrete enum dispatch.

**Rationale:**
- Vtable dispatch costs ~2-5ns. Wasm boundary crossing costs ~10-100µs. Dispatch is <0.01% of per-message cost.
- Trait objects enable `MockTransform`, `MockFilter` in tests without Wasm.
- Future extensibility (native transforms, test doubles) remains open.
- Torvyn validates: enum `WitBindings` internally, `trait ComponentInvoker` at the boundary.

---

## Decision 4: Source/Sink Remain Trait Objects (Open Set)

**Decision:** Keep `trait Source: Lifecycle` and `trait Sink: Lifecycle` as open extensible traits per ADR-0004.

**Rationale:** 4+ source types, 4+ sink types already. Users add new types (database, Kafka, gRPC). No performance benefit from closing the set — Source/Sink do I/O (network dominates).

---

## Decision 5: Node Lifecycle States — Add Recovering

**Decision:** Add `Recovering` state for InstancePre re-instantiation after `unrecoverable` errors.

```rust
pub enum NodeState {
    Starting,
    Running,
    Draining,
    Recovering,  // NEW: re-instantiating from InstancePre
    Retired,
    Error,       // terminal
}
```

**Transitions:**
```
Starting → Running → Draining → Retired
                   ↘ Recovering → Running  (re-instantiation success)
                   ↘ Error                  (terminal failure)
Recovering → Error (re-instantiation failed)
```

**Rationale:** Error policy engine's `unrecoverable` category triggers teardown + re-instantiation (~5µs via InstancePre). During this, the node must not accept messages. `Recovering` is distinct from `Error` (which is terminal).

---

## Decision 6: Instance Type Unification — Separate Bindgen Modules, Unified Store

**Decision:** Four separate `bindgen!` modules (one per Wasm world), sharing types via `with:` directive. Each `WasmX` struct wraps a shared `WasmInstance` holding `Store<WaferState>` + `WasmBindings` enum.

```rust
// crates/wafer-core/src/engine/bindings.rs
pub(crate) mod transform_world {
    wasmtime::component::bindgen!({
        path: "wit",
        world: "transform-node",
        imports: { default: async | trappable },
        exports: { default: async },
        with: { "pipeline:types/types/buffer": crate::engine::WaferBuffer },
    });
}

pub(crate) mod filter_world {
    wasmtime::component::bindgen!({
        path: "wit",
        world: "filter-node",
        imports: { default: async | trappable },
        exports: { default: async },
        with: { "pipeline:types/types": super::transform_world::pipeline::types::types },
    });
}

pub(crate) mod router_world {
    wasmtime::component::bindgen!({
        path: "wit",
        world: "router-node",
        imports: { default: async | trappable },
        exports: { default: async },
        with: { "pipeline:types/types": super::transform_world::pipeline::types::types },
    });
}

/// Internal dispatch enum — determined at instantiation time.
pub(crate) enum WasmBindings {
    Transform(transform_world::TransformNode),
    Filter(filter_world::FilterNode),
    Router(router_world::RouterNode),
}
```

**Rationale:** Torvyn uses this exact pattern successfully (separate modules with `with:` type sharing). Avoids wasmtime issue #8050 (name conflicts with multiple worlds in one module). All worlds share the same `WaferState`, `WaferBuffer`, and `Store` type.

---

## Decision 7: Native Async in Traits (RPITIT)

**Decision:** Use native `async fn` in trait methods for processing traits. Keep `Pin<Box<dyn Future>>` only for Source/Sink (which are dispatched through `dyn` at the AnyNode level and are I/O-bound).

```rust
pub trait Transform: Lifecycle {
    async fn process(&mut self, input: RuntimeEnvelope) -> Result<RuntimeEnvelope>;
}

pub trait Filter: Lifecycle {
    async fn evaluate(&mut self, input: &RuntimeEnvelope) -> Result<FilterOutcome>;
}

pub trait Router: Lifecycle {
    fn output_ports(&self) -> &[String];
    async fn route(&mut self, input: &RuntimeEnvelope) -> Result<RouteOutcome>;
}
```

**Rationale:**
- async-in-trait stable since Rust 1.75.
- Node loops call concrete types directly (monomorphized per-type loop function) — no `dyn` dispatch on the hot path.
- Eliminates one heap allocation per message for Wasm processing nodes.
- Source/Sink keep boxed futures because they ARE dispatched through `dyn` at `run_node_loop` and they're I/O-bound (alloc invisible next to network latency).

---

## Decision 8: Trait Signatures — Borrow vs Ownership Split

**Decision:** Two categories based on whether the node modifies data:

### Data Transformation (takes ownership):
```rust
/// Strict 1:1 transform. MUST produce output or return error.
pub trait Transform: Lifecycle {
    async fn process(&mut self, input: RuntimeEnvelope) -> Result<RuntimeEnvelope>;
}
```

### Flow Control (borrows input):
```rust
pub trait Filter: Lifecycle {
    async fn evaluate(&mut self, input: &RuntimeEnvelope) -> Result<FilterOutcome>;
}

pub enum FilterOutcome { Pass, Drop }

pub trait Router: Lifecycle {
    fn output_ports(&self) -> &[String];
    async fn route(&mut self, input: &RuntimeEnvelope) -> Result<RouteOutcome>;
}

pub enum RouteOutcome {
    Ports(Vec<String>),
    Drop,
}
```

### Error mapping (internal to WasmX impls):
```rust
/// Maps WIT process-error categories for the error policy engine.
#[derive(Debug)]
pub enum WasmProcessError {
    BadInput(String),
    DependencyFailed(String),
    ProcessingFailed(String),
    TimedOut,
    Unrecoverable(String),
}
```

The `WasmX` implementations map WIT results to these Rust types. The node loop's error policy engine dispatches on the category.

---

## Decision 9: Merge Is Zero-Cost DAG Topology

**Decision:** Merge is not a node. It's the DAG orchestrator wiring multiple upstream senders to one downstream receiver via standard tokio mpsc (multiple-producer, single-consumer).

**Implementation:** When the pipeline topology has edges `A→C` and `B→C`, the orchestrator gives both A and B a `Sender` clone to C's single receiver queue. Node C's loop does `recv()` and gets interleaved messages from both. No extra task, no extra queue hop, no Wasm.

**Impact on node loops:** Transform/Filter/Router loops must support receiving from a multi-sender channel (which tokio mpsc already is — it's inherently multi-producer). The current `input_receivers.into_iter().next()` pattern just takes one receiver, which already works for multi-sender.

---

## Decision 10: Fuel Budget Differentiation

**Decision:** Tighter fuel limits for flow-control nodes (Filter, Router) than data-transformation nodes (Transform).

| Node Type | Default Fuel | Rationale |
|-----------|-------------|-----------|
| Transform | 10,000,000 | Real work: JSON parsing, encoding, inference |
| Filter | 500,000 | Pure predicate — if it takes more, it's likely buggy |
| Router | 500,000 | Routing logic — metadata inspection only |

**Configurable per-node** via `[nodes.X.config]` override. Catches misbehaving plugins early (a filter that accidentally reads and processes the entire payload).

---

## Decision 11: Borrow-Only Optimization — No DLQ Pre-Clone

**Decision:** Filter and Router loops do NOT pre-clone the envelope for DLQ safety. Since they borrow the input, the original is still available after the call returns.

```rust
// Filter loop — no pre-clone needed
let envelope = recv();
match filter.evaluate(&envelope).await {
    Ok(FilterOutcome::Pass) => send_downstream(envelope),
    Ok(FilterOutcome::Drop) => record_filter_metrics(),
    Err(e) => error_policy.handle(e, envelope),  // still have it
}

// Transform loop — DLQ copy is cheap (Arc + Bytes refcount)
let envelope = recv();
let safety = envelope.clone();  // Arc bump + Bytes bump + Lineage copy (~10ns)
match transform.process(envelope).await {
    Ok(output) => send_downstream(output),
    Err(e) => error_policy.handle(e, safety),
}
```

---

## Decision 12: Router Fan-Out — Last-Port Move Optimization

**Decision:** For router fan-out, clone for N-1 ports and move the original to the last port:

```rust
let ports = router.route(&envelope).await?;
match ports.len() {
    0 => record_drop_metrics(),
    1 => send_to_port(&ports[0], envelope),           // move, zero clone
    n => {
        for port in &ports[..n-1] {
            send_to_port(port, envelope.clone());     // Arc + Bytes refcount
        }
        send_to_port(&ports[n-1], envelope);          // move original
    }
}
```

Saves one envelope clone per routed message (small but free).

---

## Type Hierarchy Summary

```
AnyNode { id, state_tracker: Arc<NodeStateTracker>, kind: NodeKind }
│
├── NodeKind::Source(Box<dyn Source>)
│     ├── MqttSource       (creates Arc<EnvelopeHeader> per message)
│     ├── FileSource
│     ├── StdinSource
│     └── HttpSource
│
├── NodeKind::Sink(Box<dyn Sink>)
│     ├── MqttSink
│     ├── FileSink
│     ├── StdoutSink
│     └── HttpSink
│
├── NodeKind::Transform(Box<dyn Transform>)
│     └── WasmTransform { engine, store, bindings: WasmBindings::Transform }
│           • Takes ownership of envelope
│           • Creates WaferBuffer, calls Wasm, deletes buffer
│           • Always produces new RuntimeEnvelope (new Arc<EnvelopeHeader>)
│
├── NodeKind::Filter(Box<dyn Filter>)
│     └── WasmFilter { engine, store, bindings: WasmBindings::Filter }
│           • Borrows envelope (&RuntimeEnvelope)
│           • Creates WaferBuffer, calls Wasm evaluate(), deletes buffer
│           • Pass → forward same envelope (Arc + Bytes refcount)
│           • Drop → nothing forwarded
│
└── NodeKind::Router(Box<dyn Router>)
      └── WasmRouter { engine, store, bindings: WasmBindings::Router, cached_ports }
            • Borrows envelope (&RuntimeEnvelope)
            • Creates WaferBuffer, calls Wasm route(), deletes buffer
            • Returns port list → host clones envelope per port (Arc refcount)

Merge: NOT a node. Multiple edges → same downstream = tokio mpsc multi-sender.
```

---

## Message Flow Through the Type System

```
MqttSource::poll()
  → Arc::new(EnvelopeHeader { id, timestamp, source, content_type, metadata })
  → RuntimeEnvelope { header: Arc<_>, payload: Bytes::from(mqtt_publish.payload), lineage: default }
  → queue.send(envelope)                                    [move into channel]

run_filter_loop:
  ← queue.recv() → envelope
  → state_tracker.set_processing(true)
  → WasmFilter::evaluate(&envelope)
      → WaferBuffer { data: envelope.payload.clone() }      [Bytes refcount, ~5ns]
      → table.push(buffer) → handle                         [~25ns]
      → construct WIT message { ..., payload: borrow<handle> }
      → bindings.call_evaluate(&mut store, &message).await  [WASM BOUNDARY]
        (guest inspects metadata, never calls buffer.read() → 0 bytes copied)
      → table.delete(handle)                                [~25ns]
      → Ok(true) → FilterOutcome::Pass
  → send_downstream(envelope)                               [move — zero clone]
  → state_tracker.set_processing(false)

run_router_loop:
  ← queue.recv() → envelope
  → WasmRouter::route(&envelope)
      → ... WIT boundary (route → ["alert", "log"]) ...
      → Ok(RouteOutcome::Ports(["alert", "log"]))
  → envelope.clone() → to "alert" port                     [Arc + Bytes refcount, ~10ns]
  → envelope → to "log" port                               [move original, 0ns]

run_transform_loop:
  ← queue.recv() → envelope
  → let safety = envelope.clone()                           [Arc + Bytes refcount for DLQ]
  → WasmTransform::process(envelope)
      → WaferBuffer { data: envelope.payload.clone() }      [Bytes refcount]
      → table.push(buffer) → handle
      → bindings.call_process(&mut store, &message).await   [WASM BOUNDARY]
        (guest calls buffer.read_all() → payload copied into linear memory)
        (guest returns output-message { payload: list<u8> } → lifted to Vec<u8>)
      → table.delete(handle)
      → Ok(RuntimeEnvelope {
          header: Arc::new(EnvelopeHeader { /* from output-message fields */ }),
          payload: Bytes::from(output_vec),                 [reuses Vec allocation]
          lineage: Lineage { parent_id: Some(input.header.id), trace_id: input.lineage.trace_id },
        })
  → send_downstream(output)

Merge (host-native):
  Multiple upstream nodes all hold Sender clones to the same channel.
  Downstream node's recv() interleaves messages from all senders.
  No extra task. No extra queue hop. No Wasm.
```

---

## Copy Count Summary

| Pipeline Shape | Payload Copies | Header Allocations |
|---------------|----------------|-------------------|
| Source → Filter → Sink | 0 | 1 (at Source) |
| Source → Router(3) → Sink | 0 | 1 (at Source) |
| Source → Transform → Sink | 2 (host→wasm, wasm→host) | 2 (Source + Transform output) |
| Source → Filter → Router(2) → Transform → Sink | 2 (only at Transform) | 2 |
| Source → Filter → Filter → Filter → Transform → Sink | 2 (only at Transform) | 2 |
| A,B → Merge → Transform → Sink | 2 per message | 1 per source + 1 per transform |

---

## Allocation Comparison

| Operation | Old (String fields) | New (Arc header) |
|-----------|-------------------|-----------------|
| Filter pass-through | ~300B allocated (string clones) | ~10ns (refcount bumps) |
| Router fan-out (3 ports) | ~900B (3× full clones) | ~30ns (3× refcount bumps) |
| Transform DLQ safety | ~300B (full clone) | ~10ns (Arc + Bytes refcount + 32B lineage) |
| Source → 5-node pipeline total allocs | ~1500B per message | ~64B per message |

---

## Future Work (Documented as Extensible)

The runtime architecture (task-per-node, bounded queues, error policy, hot-swap, InstancePre) supports all of the following without modification:

| Future Operator | WIT Interface Needed | Host Support Needed |
|----------------|---------------------|-------------------|
| Accumulate/Window | `accumulate: func(staged, list<message>) → output-message` | Timer imports, state store |
| Join | `join: func(port, message) → option<output-message>` | Windowed buffer, state store |
| Enrich | `enrich: func(message, context) → output-message` | `pipeline:host/state-store` import |
| FlatMap | `flat-map: func(message) → list<output-message>` | Bounded output enforcement |
| Dedup | Host-native (configurable seen-set) | Sliding window state |

Each requires only a new WIT world + bindgen module + node loop. The envelope, queue, error policy, hot-swap, and observability infrastructure remain unchanged.

---

## Sources Consulted

| Source | What it informed |
|--------|-----------------|
| Flink DataStream API (MapFunction, FlatMapFunction, FilterFunction) | Decision 1 (strict 1:1 Transform), Amendment A2 |
| Kafka Streams DSL (merge vs join semantics) | Amendment A1 (Merge ≠ Join) |
| Azure IoT Operations dataflow-graphs (map, filter, branch, concatenate, accumulate) | Decision 1 (operator taxonomy), Amendment A1 |
| Enterprise Integration Patterns (65 patterns: Router, Splitter, Aggregator) | Decision 1 (future work scope) |
| Torvyn source code (WitBindings enum, separate bindgen modules, task-per-flow) | Decisions 3, 6 (enum dispatch, bindgen pattern) |
| Tremor-rs (Operator trait, NodeKind enum, 10TB/day) | Decision 2 (struct + enum pattern) |
| Flow-Like (Arc<dyn NodeLogic>, blake3 cache) | Decision 3 (trait objects viable) |
| Fluvio SmartModules (filter, map, aggregate, array_map) | Decision 1 (operator categories) |
| Wick (abandoned — complexity failure) | Decision 1 (scope control matters) |
| eKuiper (goroutine-per-operator, SQL operators) | Decision 1 (IoT operator needs) |
| wasmtime issue #8050 (multiple worlds in one crate) | Decision 6 (separate modules required) |
| Web research: enum dispatch vs trait objects (2-10× on tight loops) | Decision 3 (irrelevant at Wasm call scale) |
| Web research: async-in-trait stable (Rust 1.75) | Decision 7 (native async fn) |
| Hirzel "Catalog of Stream Processing Optimizations" (2014) | Decision 1 (operator classification) |
| Lee & Parks "Dataflow Process Networks" (1995) | Amendment A2 (actor firing semantics) |
| Session 1 decisions (WIT contracts) | Amendments A1, A2 (revisiting with new evidence) |
| Session 2 decisions (host runtime) | Amendment A3 (envelope redesign) |
