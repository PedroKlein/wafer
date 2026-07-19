# ADR-0002: SPSC Bounded Queues with Backpressure

- **Date**: 2026-02-14
- **Status**: Amended
- **SPEC Reference**: Section 8.1-8.3 (Queue and Backpressure)

## Context

The DAG runtime needs inter-node communication channels. Messages flow from upstream nodes to downstream nodes through queues. The queue design affects:

1. **Throughput** - How fast messages can flow
2. **Latency** - How long messages wait in queues
3. **Memory** - How much buffering occurs
4. **Backpressure** - How overload is handled
5. **Complexity** - Implementation and debugging difficulty

### Alternatives Considered

| Approach            | Producers | Consumers | Bounded | Backpressure      |
| ------------------- | --------- | --------- | ------- | ----------------- |
| SPSC (chosen)       | 1         | 1         | Yes     | Block or drop     |
| MPSC                | N         | 1         | Yes     | Block or drop     |
| MPMC                | N         | N         | Yes     | Complex           |
| Unbounded channels  | Any       | Any       | No      | None (OOM risk)   |

### Fan-in Problem

> **Note (amended):** The original decision below assumed fan-in required explicit
> Joiner nodes. RFC-003 §A1 (Session 3) removed the Joiner node type entirely;
> fan-in is now implicit multi-producer topology (multiple upstream senders on
> one `mpsc::Receiver`). See the [Amendment (2026-07-12) — mpsc after RFC-005](#amendment-2026-07-12--mpsc-after-rfc-005)
> below for current behaviour.

Multiple upstream nodes connecting to one downstream node is a common pattern. With SPSC, this originally required an explicit **Joiner node** to merge streams.

## Decision

> **Amended:** The bullet about Joiner nodes below ("Fan-in (N→1) is handled by
> explicit Joiner nodes") was superseded by [RFC-003 §A1](../rfcs/RFC-003-node-types.md)
> — fan-in is now implicit multi-producer topology. See the
> [Amendment (2026-07-12)](#amendment-2026-07-12--mpsc-after-rfc-005) at the bottom.

Use **Single-Producer Single-Consumer (SPSC) bounded queues** for all inter-node edges.

- Each edge in the DAG has exactly one producer (upstream node) and one consumer (downstream node)
- Fan-in (N→1) is handled by explicit Joiner nodes
- Fan-out (1→N) is handled by explicit Router nodes

### Queue Implementation

Use `tokio::sync::mpsc` bounded channels with SPSC usage pattern.

**Rationale for tokio over crossbeam:**
- Native async support - no blocking on the executor
- Seamless integration with the Tokio async runtime
- Built-in backpressure via bounded capacity
- Simpler code - no need to spawn blocking tasks

The channel is technically MPSC-capable, but we use it in SPSC mode (one sender per edge). Fan-in still requires explicit Joiner nodes.

### Overflow Policies

Each edge can configure its overflow policy:

| Policy        | Behavior                           | Use Case                    |
| ------------- | ---------------------------------- | --------------------------- |
| `slow`        | Block sender until space available | Critical data, backpressure |
| `drop`        | Discard newest message, continue   | Non-critical, high volume   |
| `dead-letter` | Route dropped message to DLQ       | Debugging, recovery         |

Default: `slow` (propagate backpressure)

## Consequences

> **Amended:** the "More nodes" and "Joiner overhead" negatives, and the "Explicit
> fan-in: Joiner nodes" positive, no longer apply after [RFC-003 §A1](../rfcs/RFC-003-node-types.md).
> Fan-in is implicit multi-producer topology with zero extra nodes.

### Positive

- **Simplicity**: SPSC is the simplest queue model, easier to reason about
- **Performance**: SPSC queues can be highly optimized (no contention)
- **Explicit fan-in**: Joiner nodes make merge behavior visible and configurable
- **Bounded memory**: No unbounded growth, predictable memory usage
- **Clear backpressure**: Slow policy propagates pressure to source

### Negative

- **More nodes**: Fan-in requires explicit Joiner nodes (more config)
- **Joiner overhead**: Extra hop for merged streams
- **Policy choice**: Users must choose appropriate overflow policy per edge

### Neutral

- **crossbeam dependency**: Well-maintained, but adds a dependency
- **Configuration verbosity**: Each edge needs queue config (capacity, policy)

## Open Questions Resolved

This decision relates to the pre-refactor SPEC.md (retired) Section 18.2:
> **Queue implementation:** crossbeam-channel vs custom ring buffer?

**Resolution**: Start with crossbeam-channel, profile and optimize if needed.

---

## Amendment (2026-07-12) — mpsc after RFC-005

- **Amending decision**: [RFC-005 Orchestrator & Runtime Simplification](../rfcs/RFC-005-orchestrator.md), Session 5
- **Nature of change**: Supersedes the SPSC framing; edges are now multi-producer single-consumer (mpsc)

### Context

RFC-003 (Session 3) eliminated the Joiner node entirely — fan-in is expressed
as multiple producers sharing the receiver's single `mpsc` channel end. RFC-005
(Session 5) confirmed the runtime uses `tokio::sync::mpsc::channel` directly,
removing the `BoundedQueue` wrapper crate that previously imposed SPSC
semantics on top of the underlying mpsc primitive.

### What changed

1. **SPSC → mpsc.** Each edge is still one `Sender` clone, but merge edges
   share the same `Receiver` via sender clones. The queue is bounded mpsc, not
   bounded SPSC.
2. **BoundedQueue removed.** The runtime calls `tokio::sync::mpsc::channel`
   directly. Queue wiring uses a receiver-keyed map: one channel per
   destination node (each node has a single default input port), with
   sender clones for merge edges.
3. **Joiner node removed.** Fan-in is implicit multi-producer on the
   receiver's channel — no extra hop, no extra config. This follows RFC-003
   amendment A1.
4. **Overflow policies unchanged.** `slow` / `drop` / `dead-letter` remain
   per-edge, applied by the sender-side logic in the runner loop (before
   `send().await`).
5. **Capacity conflict on merge.** When two edges target the same receiver
   with different configured capacities, the maximum is used.

### Consequences of this amendment

- The "More nodes" and "Joiner overhead" negatives listed above are eliminated.
- Fan-in is zero-cost (mpsc multi-sender is a refcount bump on the channel
  internals — no extra task, no extra channel hop).
- The SPSC performance advantage (no contention) is traded for simpler topology
  at negligible cost — tokio mpsc send/recv is ~50-100 ns on RPi 4, which is
  <2% of per-hop Wasm boundary crossing cost.
- Configuration is simpler: users declare edges; merge is inferred from
  topology.
- crossbeam-channel dependency is no longer used for inter-node queues.

### See Also

- [RFC-005 — Orchestrator & Runtime Simplification](../rfcs/RFC-005-orchestrator.md) — the long-form decision that motivates this amendment (D1, D2, D9).
- [RFC-003 — Node Type Architecture](../rfcs/RFC-003-node-types.md) — removes Joiner (A1), defines merge as multi-producer mpsc (D9).
- [ADR-0003](0003-hot-swap-mechanism.md) — hot-swap mechanism (rewritten; original was `0003-drain-and-flip-hotswap.md`, superseded by RFC-005's watch-channel model).
