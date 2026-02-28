# ADR-0002: SPSC Bounded Queues with Backpressure

- **Date**: 2026-02-14
- **Status**: Accepted
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

Multiple upstream nodes connecting to one downstream node is a common pattern. With SPSC, this requires an explicit **Joiner node** to merge streams.

## Decision

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

This decision relates to SPEC.md Section 18.2:
> **Queue implementation:** crossbeam-channel vs custom ring buffer?

**Resolution**: Start with crossbeam-channel, profile and optimize if needed.
