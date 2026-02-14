# ADR-0003: Drain-and-Flip Hot-Swap Mechanism

- **Date**: 2026-02-14
- **Status**: Accepted
- **SPEC Reference**: Section 10.1-10.3 (Hot-Swap Mechanism)

## Context

The runtime must support upgrading individual nodes without stopping the entire pipeline. This is critical for:

1. **Zero-downtime upgrades** - Update node logic without service interruption
2. **Bug fixes** - Patch nodes in production
3. **A/B testing** - Gradually roll out new versions
4. **Configuration changes** - Update node config without restart

### Alternatives Considered

| Approach        | Message Loss | Pause Duration | Complexity | State Handling |
| --------------- | ------------ | -------------- | ---------- | -------------- |
| Stop-and-restart| High         | Long           | Low        | Lost           |
| Blue-green      | None         | Medium         | High       | Complex        |
| Drain-and-flip  | Near-zero    | Short          | Medium     | Stateless only |
| Shadow mode     | None         | None           | Very high  | Complex        |

## Decision

Use **Drain-and-Flip** as the hot-swap mechanism.

### Algorithm

```
1. PREPARE
   ├─ Load new component (v2.wasm)
   ├─ Instantiate v2 in Wasmtime
   └─ Call v2.validate() and v2.init()

2. DRAIN
   ├─ Mark v1 as "draining"
   ├─ Stop routing NEW messages to v1
   ├─ Wait for v1's in-flight messages to complete
   └─ (Timeout: drain_timeout_ms, default 5000ms)

3. FLIP
   ├─ Atomically swap: route new messages to v2
   └─ v2 is now active

4. RETIRE
   ├─ Call v1.close()
   ├─ Drop v1 instance
   └─ Free v1 resources
```

### Failure Handling

| Failure             | Recovery                                    |
| ------------------- | ------------------------------------------- |
| v2 validation fails | Abort swap, keep v1 running                 |
| v2 init fails       | Abort swap, keep v1 running                 |
| Drain timeout       | Force swap, log dropped message count       |
| v2 crashes after    | Restart v2 (no automatic rollback for now)  |

### Scope Limitation

This mechanism is for **stateless nodes only**. Stateful hot-swap (with state migration) is future work.

## Consequences

### Positive

- **Near-zero message loss**: Only messages in-flight during flip
- **Short pause**: Typically < 100ms under normal load
- **Simple mental model**: Drain old, flip to new
- **Safe rollback point**: If v2 fails validation, v1 continues unchanged
- **No duplicate processing**: Messages go to v1 OR v2, never both

### Negative

- **Drain timeout risk**: Under high load, drain may timeout and force message loss
- **No state migration**: Stateful nodes lose state during swap
- **Brief pause**: New messages wait during flip (not truly zero-latency)
- **No automatic rollback**: If v2 fails after swap, manual intervention needed

### Neutral

- **Metrics overhead**: Need to track swap timing, delayed/dropped messages
- **Configuration**: drain_timeout_ms must be tuned per deployment

## Metrics to Collect

| Metric                    | Description                      |
| ------------------------- | -------------------------------- |
| `swap_prepare_time_ns`    | Time to load and initialize v2   |
| `swap_drain_time_ns`      | Time waiting for v1 to drain     |
| `swap_flip_time_ns`       | Time for atomic swap operation   |
| `swap_total_time_ns`      | Total hot-swap duration          |
| `swap_messages_in_flight` | Messages in v1 at drain start    |
| `swap_messages_delayed`   | Messages that waited during swap |
| `swap_messages_dropped`   | Messages dropped (drain timeout) |

## Future Work

- **Snapshot/restore**: For stateful nodes, capture state from v1 and restore to v2
- **Automatic rollback**: If v2 fails repeatedly, automatically revert to v1
- **Canary deployment**: Route percentage of traffic to v2 before full flip
