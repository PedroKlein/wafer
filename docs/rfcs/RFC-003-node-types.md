# RFC-003: Node Type Architecture

- **Status:** Implemented
- **Original session date:** 2026-07-06
- **Amends:** RFC-001 (§A1 removes Joiner world; §A2 Transform strict 1:1), RFC-002 (§A3 RuntimeEnvelope redesigned with Arc header)

## Abstract

This RFC designs the host-side node type system — the Rust types, traits, and dispatch patterns that realize the WIT contracts defined in RFC-001. It revisits the operator taxonomy from first principles, producing three amendments to prior RFCs: removing the Joiner WIT world (merge is a zero-cost host topology operation via multi-producer mpsc), simplifying Transform to strict 1:1 return semantics, and redesigning RuntimeEnvelope with an Arc-wrapped immutable header for near-free cloning during fan-out and DLQ safety copies. The RFC establishes five node categories (Source, Sink, Transform, Filter, Router), a struct-with-inner-enum dispatch pattern (`AnyNode` + `NodeKind`), trait-object processing interfaces, ownership-vs-borrow trait signature differentiation, fuel budget differentiation by node category, and router fan-out move optimization. Merge is explicitly not a node — it is expressed as multiple senders to one tokio mpsc receiver.

## Context

With WIT contracts (RFC-001) and host runtime architecture (RFC-002) decided, this session designed the node type system. Key inputs:

- **4 WIT worlds from RFC-001**: `transform-node`, `filter-node`, `router-node`, `joiner-node` (the last subsequently removed by this RFC)
- **Task-per-node architecture** with bounded mpsc channels
- **`borrow<buffer>` input**, `list<u8>` output (asymmetric payload crossing)
- **5-category process-error** with per-node error policy (RFC-002 §D4)
- **InstancePre** for hot-swap fast instantiation (RFC-002 §D9)
- **Persistent Store per node** (RFC-002 §D10)

The session also revisited the operator taxonomy from first principles, using evidence from Kafka Streams (merge ≠ join), Azure IoT Operations (concatenate is built-in topology), and Flink (`MapFunction` is strict 1:1). This produced amendments to RFC-001 and RFC-002.

## Decisions

### Amendment A1: Remove Joiner WIT World

Remove the `joiner-node` world and `joiner` interface entirely. Merge is a host-native topology operation — not a Wasm plugin.

The stream processing literature distinguishes two fundamentally different N→1 operations:

| | **Merge** | **Join** |
|---|-----------|---------|
| What it does | Interleaves N streams into 1 | Correlates records, produces derived output |
| Logic needed | NONE — pure topology | YES — correlation key, time window, combine function |
| Creates new records? | No — preserves upstream records unchanged | Yes — new derived records |
| Stateful? | No | Yes (buffers records waiting for match) |

WAFER's "Joiner" was conflating both. Merge needs no Wasm — it is tokio mpsc (multiple producers, single consumer). Join requires windowed state — explicitly out of thesis scope.

**Evidence from comparators:** Kafka Streams has `merge()` (no logic) completely separate from `join()` (correlate + derive). Azure IoT's `concatenate` operator needs no Wasm module — it is built-in topology. Flink's Union is a topology operation, not a user-defined operator.

**Impact:** `pipeline:routing@0.1.0` loses the `joiner` interface and `joiner-node` world. Three WIT worlds remain: `transform-node`, `filter-node`, `router-node`. *(Amends RFC-001.)*

### Amendment A2: Transform Is Strict 1:1

Transform MUST always return an `output-message` or an error. The `filter` variant is removed from `process-outcome`. The WIT return type simplifies to `result<output-message, process-error>`.

Rationale: Flink's `MapFunction` is strict 1:1 ("A Map function always produces a single result element for each input element"). With Filter as a first-class node type, Transform no longer needs the degenerate "I chose not to emit" path. If a component cannot process input, it returns an error (`bad-input` category → DLQ). Pipeline designers place Filter upstream for validation.

**Impact:** `process-outcome` variant is removed from `pipeline:types`. Transform return is now `result<output-message, process-error>` directly. *(Amends RFC-001.)*

### Amendment A3: RuntimeEnvelope Redesigned with Arc Header

Split RuntimeEnvelope into immutable header (Arc-wrapped), payload (Bytes), and mutable lineage. Filter and Router borrow the envelope — they never modify it. When they pass through or fan-out, the host clones the envelope for each downstream queue. With the old design this cloned every string field (~200–400 bytes of allocations per clone). With Arc:

- Filter pass-through: Arc refcount bump + Bytes refcount bump = ~16 bytes vs ~300B before
- Router fan-out to 3 ports: 3 refcount bumps = 0 bytes allocated vs ~900B before
- Transform DLQ safety copy: Arc refcount bump = near-free vs full clone before

**Updated RuntimeEnvelope:**
```rust
pub struct RuntimeEnvelope {
    pub header: Arc<EnvelopeHeader>,   // immutable message identity, shared via refcount
    pub payload: Bytes,                // refcounted, zero-copy from MQTT source
    pub(crate) lineage: Lineage,       // mutable per-hop tracking
}
```

Clone cost: Arc increment (~5ns) + Bytes increment (~5ns) + Lineage copy (~32B) = ~10ns total. *(Amends RFC-002.)*

### Decision 1: Node Type Set — 4 Types in Two Categories

WAFER has exactly 4 node types in two categories:

**Category 1 — Data Transformation** (ownership, produces new bytes):
- **Transform** (Wasm): Strict 1:1. Takes message, MUST return new output-message.

**Category 2 — Flow Control** (borrow-only, zero-copy possible):
- **Filter** (Wasm): Pure predicate. Borrows message, returns bool.
- **Router** (Wasm): Routing decision. Borrows message, returns port list.
- **Merge** (Host-native): Topology operation. Multiple inputs → one output. No Wasm.

Not included (documented as future work): Aggregate/Window, Join, FlatMap/Splitter, Enrich, Dedup.

### Decision 2: AnyNode Structure — Struct with Inner Enum

Extract state tracker and node identity into a shared wrapper struct. The polymorphic behavior lives in `NodeKind`.

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

### Decision 3: Keep Trait Objects for Processing Nodes

Keep `dyn Transform`, `dyn Filter`, `dyn Router` — do NOT switch to concrete enum dispatch. Vtable dispatch costs ~2–5ns; Wasm boundary crossing costs ~10–100µs. Dispatch is <0.01% of per-message cost. Trait objects enable `MockTransform`, `MockFilter` in tests without Wasm.

### Decision 4: Source/Sink Remain Trait Objects (Open Set)

Keep `trait Source: Lifecycle` and `trait Sink: Lifecycle` as open extensible traits per ADR-0004. 4+ source types, 4+ sink types already exist. Users add new types (database, Kafka, gRPC). No performance benefit from closing the set — Source/Sink do I/O where network dominates.

### Decision 5: Node Lifecycle States — Add Recovering

Add `Recovering` state for InstancePre re-instantiation after `unrecoverable` errors:

```rust
pub enum NodeState {
    Starting, Running, Draining, Recovering, Retired, Error,
}
```

Transitions: `Starting → Running → Draining → Retired`; `Running → Recovering → Running` (success); `Running → Error` or `Recovering → Error` (terminal).

### Decision 6: Instance Type Unification — Separate Bindgen Modules, Unified Store

Four separate `bindgen!` modules (one per Wasm world), sharing types via `with:` directive. Each `WasmX` struct wraps a shared `WasmInstance` holding `Store<WaferState>` + `WasmBindings` enum.

```rust
pub(crate) mod transform_world { wasmtime::component::bindgen!({ world: "transform-node", ... }); }
pub(crate) mod filter_world   { wasmtime::component::bindgen!({ world: "filter-node", ... }); }
pub(crate) mod router_world   { wasmtime::component::bindgen!({ world: "router-node", ... }); }

pub(crate) enum WasmBindings { Transform, Filter, Router }
```

Avoids wasmtime issue #8050 (name conflicts with multiple worlds in one module). All worlds share the same `WaferState`, `WaferBuffer`, and `Store` type.

### Decision 7: Native Async in Traits (RPITIT)

Use native `async fn` in trait methods for processing traits. Source/Sink keep boxed futures because they are dispatched through `dyn` at `run_node_loop` and are I/O-bound (alloc invisible next to network latency).

### Decision 8: Trait Signatures — Borrow vs Ownership Split

Two categories based on whether the node modifies data:

- **Transform** takes ownership: `async fn process(&mut self, input: RuntimeEnvelope) -> Result<RuntimeEnvelope>`
- **Filter** borrows: `async fn evaluate(&mut self, input: &RuntimeEnvelope) -> Result<FilterOutcome>`
- **Router** borrows: `async fn route(&mut self, input: &RuntimeEnvelope) -> Result<RouteOutcome>`

Error mapping uses 5-category `WasmProcessError` for the error policy engine.

### Decision 9: Merge Is Zero-Cost DAG Topology

Merge is not a node. The DAG orchestrator wires multiple upstream senders to one downstream receiver via standard tokio mpsc (multiple-producer, single-consumer). When edges `A→C` and `B→C` exist, both A and B hold a `Sender` clone to C's single receiver. Node C's loop calls `recv()` and gets interleaved messages.

### Decision 10: Fuel Budget Differentiation

Tighter fuel limits for flow-control nodes than data-transformation nodes:

| Node Type | Default Fuel | Rationale |
|-----------|-------------|-----------|
| Transform | 10,000,000 | Real work: JSON parsing, encoding, inference |
| Filter | 500,000 | Pure predicate — if more, likely buggy |
| Router | 500,000 | Routing logic — metadata inspection only |

Configurable per-node via `[nodes.X]` override.

### Decision 11: Borrow-Only Optimization — No DLQ Pre-Clone

Filter and Router loops do NOT pre-clone the envelope for DLQ safety. Since they borrow the input, the original is still available after the call returns. Transform pre-clones cheaply (Arc + Bytes refcount, ~10ns).

### Decision 12: Router Fan-Out — Last-Port Move Optimization

For router fan-out, clone for N-1 ports and move the original to the last port — saves one envelope clone per routed message.

## Alternatives Considered

1. **Keep Joiner as a Wasm node type** — Rejected because merge is a pure topology operation needing zero logic; join requires windowed state which is explicitly out of thesis scope. Separate concepts conflated as one node type.
2. **Allow Transform to return "filter" / skip variant** — Rejected because Filter exists for dropping. Strict 1:1 gives the host a branchless happy path and makes plugin DX unambiguous.
3. **Enum dispatch instead of trait objects for processing nodes** — Rejected because vtable cost (~2–5ns) is negligible vs Wasm boundary (~10–100µs), and trait objects enable mocking in tests without Wasm.
4. **Full-clone envelope (no Arc header)** — Rejected because filter pass-through and router fan-out would allocate ~300B per clone instead of ~10ns refcount bumps. The Arc + Bytes design reduces per-message allocations from ~1500B to ~64B in a 5-node pipeline.
5. **Shared fuel budget across all Wasm node types** — Rejected because flow-control nodes (filter, router) should not consume the same budget as data-transformation nodes. Tighter limits catch misbehaving plugins early.
6. **Pre-clone envelope for DLQ in all node loops** — Rejected because borrow-only nodes (filter, router) still hold the original after the call returns. Only transform needs the safety copy.

## Related RFCs

- **RFC-001** — Defines the WIT contracts that this RFC's type system realizes. This RFC amends RFC-001 §A1 (removes Joiner world) and §A2 (Transform strict 1:1 return).
- **RFC-002** — Defines the host runtime architecture (RuntimeEnvelope, error policy, InstancePre, Store lifecycle). This RFC amends RFC-002 §A3 (envelope redesigned with Arc header + Bytes + Lineage).
- **RFC-005** — Later simplifies the orchestrator; builds on the node type set established here.
- **RFC-006** — Guest SDK design for the trait interfaces defined here.

## Implementation Notes

- `NodeKind` enum is implemented at `crates/wafer-core/src/node/kind.rs` as a simple discriminator (not carrying trait objects) — the runtime dispatches per-type node loops directly rather than through a unified `NodeKind` enum carrying `Box<dyn Trait>`. The logical intent (5 kinds, no Joiner) matches the decision.
- `WasmBindings` at `crates/wafer-core/src/engine/bindings.rs` implements Decision 6 with three variants (`Transform`, `Filter`, `Router`) — no Joiner variant.
- The three `bindgen!` modules (`transform_world`, `filter_world`, `router_world`) exist in `crates/wafer-core/src/engine/bindings.rs` with `with:` type sharing as specified.
- RuntimeEnvelope with `Arc<EnvelopeHeader>` + `Bytes` + `Lineage` is implemented as decided in Amendment A3.
- Merge is realized as multi-producer `mpsc` wiring in the orchestrator builder (`crates/wafer-core/src/orchestrator/builder.rs`); multiple upstream nodes receive `Sender` clones to the downstream node's single receiver.
- The WIT files (`wit/pipeline-node.wit`, `wit/pipeline-routing.wit`) contain only `transform-node`, `filter-node`, `inference-node`, `router-node` — no `joiner-node` world.
- `inference-node` world was added post-RFC as a Transform variant for `wasi:nn` plugins; it is functionally a Transform with additional WASI imports and does not conflict with any decision here.
- Code matches decisions; no divergence requiring reconciliation.
