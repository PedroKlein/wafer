# ADR-0010: Merge as Host-Native Topology

- **Date**: 2026-07-06
- **Status**: Accepted
- **Parent RFC**: [RFC-003](../rfcs/RFC-003-node-types.md)

## Context

WAFER's original design included a `joiner-node` WIT world — a Wasm plugin type responsible for combining multiple upstream streams into one downstream. This conflated two fundamentally different N→1 operations: *merge* (interleave streams unchanged) and *join* (correlate records across streams using keys, windows, or watermarks). The stream processing literature treats these as distinct concepts: Kafka Streams exposes `merge()` as a zero-logic topology primitive separate from its stateful `join()` operators; Azure IoT Operations provides a built-in `concatenate` operator that requires no user code; Flink's `Union` is a topology instruction, not a user-defined function. Meanwhile, WAFER's inter-node transport already uses tokio `mpsc` channels (multiple-producer, single-consumer) — which naturally interleave messages from multiple senders into one receiver without any additional logic.

Running merge through a Wasm boundary adds per-message overhead (~10–100µs for the Wasm call, canonical ABI lifts, fuel accounting) to an operation that can be achieved at zero cost through channel wiring alone. ADR-0002 documented SPSC usage of mpsc channels and specified that fan-in "requires explicit Joiner nodes." RFC-003 §A1 and §D9 supersede that position: merge becomes implicit multi-producer wiring.

## Decision

We remove the `joiner-node` WIT world and express merge as host-native DAG topology. When multiple upstream nodes connect to the same downstream node, the orchestrator builder (`crates/wafer-core/src/orchestrator/builder.rs` — `wire_queues`) creates a single `mpsc::channel` for the destination and distributes `Sender` clones to each upstream node. The downstream node's runner loop calls `recv()` and processes messages as they arrive from any upstream sender — interleaving is handled by tokio's mpsc implementation with no additional code, no Wasm invocation, and no per-message overhead.

## Consequences

- **Positive — Zero runtime cost:** Merge adds no task, no Wasm call, no fuel consumption, no allocation beyond the channel wiring at pipeline startup. An N-way merge has identical per-message latency to a 1-to-1 edge.

- **Positive — Reduced surface area:** One fewer WIT world to maintain, one fewer bindgen module, one fewer runner-loop variant. The WIT interface set shrinks from four worlds to three (`transform-node`, `filter-node`, `router-node`).

- **Positive — Simpler pipeline configuration:** Users do not declare or configure merge nodes. Multiple edges pointing at the same downstream node in the `[[edges]]` config table implicitly express a merge. Validation at build time confirms only Transform and Sink nodes may have multiple inputs (per RFC-004).

- **Positive — Backpressure preserved automatically:** A single bounded mpsc channel with multiple senders means all upstream paths share the same capacity limit. If the downstream node is slow, senders from all upstream paths block (or overflow per their configured policy) — no custom backpressure logic needed.

- **Positive — No message ordering guarantees needed at merge point:** By using tokio mpsc's natural interleaving, message ordering is determined by arrival timing. This matches the semantics users expect from a simple stream merge.

- **Negative / trade-off — No ordering guarantees across branches:** Messages from branch A and branch B arrive in an undefined interleaving order at the merge point. If a user needs deterministic ordering (e.g. by timestamp, sequence number), they must implement sorting logic inside the downstream node. The framework provides no built-in ordering facility for merged streams.

- **Forecloses — Stateful joins:** Windowed aggregation (tumbling, sliding, session windows), key-based correlation joins, and watermark-based ordering are all out of scope. These require buffering records from multiple streams until a correlation condition is met — fundamentally stateful operations that cannot be expressed as passive channel wiring. Users needing join semantics must implement them inside a downstream Transform plugin that maintains its own state via the guest SDK's `thread_local!` + `RefCell` pattern.

- **Forecloses — Per-source fairness policies:** With passive mpsc interleaving, a fast upstream producer can starve a slow one at the merge point. Weighted fair-queuing, round-robin scheduling, or priority-based merging are not available without introducing a dedicated merge task.

- **Forecloses — Merge-point metadata injection:** No node exists at the merge point, so there is no opportunity to stamp messages with a "source branch" identifier at merge time. If downstream logic needs to know which branch a message came from, the upstream node must already carry that information in the envelope header or metadata.

- **Downstream requirement — ADR-0002 amendment:** ADR-0002 documents SPSC usage and "explicit Joiner nodes for fan-in." That text is superseded by this decision; ADR-0002 should carry an amendment note pointing here.

- **Downstream requirement — Config validation:** The pipeline builder must validate that only Transform and Sink nodes accept multiple inbound edges (per RFC-004 §D3). Sources and flow-control nodes (Filter, Router) with multiple inputs represent a configuration error.

## See Also

- [RFC-003](../rfcs/RFC-003-node-types.md) — Long-form decision (§A1: Remove Joiner world; §D9: Merge is zero-cost DAG topology).
- [RFC-001](../rfcs/RFC-001-wit-contracts.md) — Original WIT contracts; the `joiner-node` world and `joiner` interface removed by RFC-003 §A1.
- [ADR-0002](0002-spsc-bounded-queues.md) — SPSC bounded queues; fan-in section superseded by this decision.
- [RFC-005](../rfcs/RFC-005-orchestrator.md) — Orchestrator design that builds on multi-sender merge wiring.
- `crates/wafer-core/src/orchestrator/builder.rs` — `wire_queues` function creates one receiver per destination, clones the sender per source edge.
- `wit/pipeline-routing.wit` — Contains only `router-node` world; no `joiner-node`.
