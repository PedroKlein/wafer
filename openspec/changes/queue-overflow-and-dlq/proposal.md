## Why

The current queue implementation only supports `slow` (blocking) backpressure policy. When a queue fills up, the sender blocks indefinitely, propagating backpressure upstream. While this is correct for critical data flows, it's insufficient for real-world edge scenarios where:

1. **Non-critical data should be droppable** - telemetry data that's stale if delayed should be dropped rather than blocking the entire pipeline
2. **Failed messages need recovery paths** - messages that can't be processed should be routed to a Dead Letter Queue (DLQ) for debugging, replay, or alternative handling
3. **Sinks need batching for efficiency** - MQTT/file sinks should batch messages to reduce I/O overhead

This aligns with SPEC §8.2 (Overflow Policies) which defines `slow`, `drop`, and `dead-letter` policies, and SPEC §4.9 (Sink Features) which specifies `batch_size`, `batch_timeout`, and `flush()` requirements.

## What Changes

- **Queue overflow policies**:
  - Add `OverflowPolicy` enum with `Slow`, `Drop`, `DeadLetter` variants
  - Add `overflow` field to `EdgeDefinition` in config schema
  - Implement `drop` policy: discard newest message when queue is full, increment counter
  - Implement `dead-letter` policy: route dropped messages to configured DLQ sink

- **Dead Letter Queue (DLQ)**:
  - Add `[dead_letter]` config section for pipeline-wide DLQ configuration
  - DLQ sink receives `RuntimeEnvelope` wrapped with error context (original edge, reason, timestamp)
  - DLQ can be any sink type (file, MQTT, stdout for debugging)

- **Sink batching**:
  - Add `batch_size` and `batch_timeout_ms` config options to sink nodes
  - Implement internal buffer in sinks that flushes on batch size OR timeout
  - Add `flush()` method to `Sink` trait for explicit buffer flush
  - Call `flush()` before `close()` during shutdown

- **Metrics**:
  - Add `queue_drop_total` counter per edge (for `drop` policy)
  - Add `queue_dlq_total` counter per edge (for `dead-letter` policy)
  - Add `sink_batch_flush_total` counter
  - Add `sink_batch_size` histogram

## Capabilities

### New Capabilities
- `queue-overflow-policies`: Configurable overflow handling (`slow`, `drop`, `dead-letter`) per edge with metrics
- `dead-letter-queue`: Pipeline-wide DLQ sink for failed/dropped messages with error context
- `sink-batching`: Configurable batch buffering for sinks with size/timeout triggers

### Modified Capabilities
<!-- No existing specs to modify - these are new capabilities -->

## Impact

**Code changes**:
- `crates/wafer-core/src/queue/bounded.rs` - Add overflow policy support to `QueueSender`
- `crates/wafer-core/src/config/schema.rs` - Add `OverflowPolicy`, `DeadLetterConfig`, sink batch fields
- `crates/wafer-core/src/node/sink/mod.rs` - Add `flush()` to trait, batching support
- `crates/wafer-core/src/node/sink/*.rs` - Implement batching in FileSink, MqttSink
- `crates/wafer-core/src/dag/builder.rs` - Wire DLQ sink, respect overflow policies
- `crates/wafer-core/src/dag/runner.rs` - Handle overflow during send
- `crates/wafer-core/src/metrics/` - Add new counters and histograms

**Config format** (non-breaking additions):
```toml
[dead_letter]
enabled = true
sink_type = "file"
[dead_letter.config]
path = "/var/log/wafer/dlq.jsonl"

[[edges]]
from = "source"
to = "transform"
queue_capacity = 1000
overflow = "drop"  # NEW: "slow" (default), "drop", "dead-letter"

[[nodes]]
id = "mqtt-sink"
node_type = "sink"
sink_type = "mqtt"
[nodes.config]
broker = "tcp://localhost:1883"
topic = "output"
batch_size = 100        # NEW: flush after 100 messages
batch_timeout_ms = 500  # NEW: flush after 500ms regardless of count
```

**Dependencies**: 
- `chrono = { version = "0.4", features = ["serde"] }` - for DLQ envelope timestamps

**SPEC alignment**: Implements SPEC §8.2 (Overflow Policies), §7.2 (DLQ), §4.9 (Sink batching)
