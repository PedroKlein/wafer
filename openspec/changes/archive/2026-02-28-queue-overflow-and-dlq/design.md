## Context

The WAFER pipeline runtime currently implements only the `slow` (blocking) backpressure policy. When a bounded queue fills up, the `QueueSender::send()` method blocks asynchronously until space becomes available. While this is semantically correct for critical data flows, it's insufficient for real-world edge scenarios:

**Current State:**
- `BoundedQueue<T>` wraps `tokio::sync::mpsc` with blocking `send()` semantics
- `EdgeDefinition` has `queue_capacity` but no overflow policy configuration
- `Sink` trait has only `collect()` - no batching or explicit flush mechanism
- DAG runner loops call `sender.send(envelope).await` which blocks on full queues

**Constraints:**
- Must remain async-native (no blocking the tokio runtime)
- WASM cancel safety rules still apply (no WASM calls inside `select!`)
- DLQ must handle any `RuntimeEnvelope` without knowing its schema
- Batching timeout must not starve - flush even if batch isn't full
- Changes must be backward-compatible (existing configs without `overflow` should default to `slow`)

## Goals / Non-Goals

**Goals:**
- Implement `drop` and `dead-letter` overflow policies alongside existing `slow`
- Create a pipeline-wide Dead Letter Queue sink for dropped/failed messages
- Add batching support to sinks with size and timeout triggers
- Expose metrics for overflow events and batch operations
- Maintain full backward compatibility with existing configurations

**Non-Goals:**
- Per-message retry logic (beyond routing to DLQ)
- Complex DLQ routing rules (single pipeline-wide DLQ is sufficient)
- Sink-specific DLQ (one global DLQ handles all failures)
- Persistent DLQ storage guarantees (DLQ is just another sink)
- Back-off or circuit breaker patterns (future enhancement)

## Decisions

### Decision 1: Overflow Policy Implementation Location

**Chosen:** Implement overflow handling in the DAG runner loops, not in `QueueSender`.

**Rationale:**
- The runner loops already have access to metrics registry and control state
- DLQ routing requires access to the DLQ sender, which is orchestrator-level state
- `try_send()` already exists on `QueueSender` for non-blocking attempts
- Keeps queue module simple and reusable

**Alternatives Considered:**
- *Wrap `QueueSender` with policy-aware sender*: Would require threading DLQ through queue module, mixing concerns
- *Policy enum on `QueueSender::send()`*: Would complicate the queue API for a single use case

**Implementation:**
```rust
// In runner.rs, replace direct send() calls:
match overflow_policy {
    OverflowPolicy::Slow => sender.send(envelope).await?,
    OverflowPolicy::Drop => {
        if let Err(TrySendError::Full(_)) = sender.try_send(envelope) {
            metrics.increment_drop_counter(edge_id);
            // message dropped
        }
    }
    OverflowPolicy::DeadLetter => {
        if let Err(TrySendError::Full(envelope)) = sender.try_send(envelope) {
            metrics.increment_dlq_counter(edge_id);
            dlq_sender.send(wrap_dlq_envelope(envelope, edge_id, "queue_full")).await?;
        }
    }
}
```

### Decision 2: DLQ Envelope Wrapping

**Chosen:** Wrap failed messages in a `DlqEnvelope` that preserves the original `RuntimeEnvelope` plus error context.

**Rationale:**
- Preserves all original message data for debugging/replay
- Error context (edge, reason, timestamp) aids diagnosis
- JSON-serializable for file/MQTT sinks
- Can be unwrapped for replay scenarios

**Structure:**
```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DlqEnvelope {
    /// Original message that failed
    pub original: RuntimeEnvelope,
    /// Edge where failure occurred
    pub failed_edge: String,
    /// Reason for failure
    pub reason: DlqReason,
    /// When the failure occurred
    pub failed_at: chrono::DateTime<chrono::Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum DlqReason {
    QueueFull,
    ProcessError { code: String, message: String },
    SinkError { message: String },
}
```

**Alternatives Considered:**
- *Flatten into RuntimeEnvelope metadata*: Loses type safety, harder to query
- *Separate error channel*: Over-engineered for MVP needs

### Decision 3: DLQ as Special Sink

**Chosen:** DLQ is configured as a special sink in the `[dead_letter]` config section, instantiated once and shared across the orchestrator.

**Rationale:**
- Reuses existing sink infrastructure (FileSink, MqttSink, StdoutSink)
- Single configuration point for all DLQ routing
- Can be disabled entirely with `enabled = false`
- DLQ sink receives `DlqEnvelope` serialized as JSON payload

**Configuration:**
```toml
[dead_letter]
enabled = true
sink_type = "file"
[dead_letter.config]
path = "/var/log/wafer/dlq.jsonl"
```

**Alternatives Considered:**
- *Per-edge DLQ configuration*: Over-complicated for MVP
- *DLQ as a node in the DAG*: Circular dependency issues, harder to reason about

### Decision 4: Batching Architecture

**Chosen:** Internal buffer in each sink with a `BatchBuffer<RuntimeEnvelope>` helper, flushed on size OR timeout.

**Rationale:**
- Encapsulates batching logic in reusable component
- Timeout prevents starvation (flush even if batch not full)
- `flush()` trait method enables explicit flush before shutdown
- Works with existing `collect()` method (buffer internally, flush when ready)

**Design:**
```rust
pub struct BatchBuffer<T> {
    buffer: Vec<T>,
    batch_size: usize,
    timeout: Duration,
    last_flush: Instant,
}

impl<T> BatchBuffer<T> {
    pub fn push(&mut self, item: T) -> Option<Vec<T>> {
        self.buffer.push(item);
        if self.buffer.len() >= self.batch_size || self.last_flush.elapsed() >= self.timeout {
            Some(self.take())
        } else {
            None
        }
    }
    
    pub fn take(&mut self) -> Vec<T> {
        self.last_flush = Instant::now();
        std::mem::take(&mut self.buffer)
    }
}

// Updated Sink trait
pub trait Sink: Lifecycle {
    fn collect(&mut self, envelope: RuntimeEnvelope) -> Pin<Box<dyn Future<Output = Result<()>> + Send + '_>>;
    
    /// Flush any buffered messages. Called before close().
    fn flush(&mut self) -> Pin<Box<dyn Future<Output = Result<()>> + Send + '_>> {
        // Default: no-op for non-batching sinks
        Box::pin(async { Ok(()) })
    }
}
```

**Alternatives Considered:**
- *External batching wrapper*: Would require wrapping every sink, more complex
- *Batching in runner loop*: Mixes concerns, harder to test sinks in isolation

### Decision 5: Batch Timeout Mechanism

**Chosen:** Check timeout on each `collect()` call, plus add a background timer task in the sink loop.

**Rationale:**
- Timeout check on collect() handles high-throughput cases efficiently
- Background timer ensures flush even during idle periods
- No additional threads needed (tokio timer integration)

**Implementation in runner:**
```rust
// In run_sink_loop, add timeout arm to select
let flush_interval = sink.batch_timeout().unwrap_or(Duration::MAX);
let mut flush_timer = tokio::time::interval(flush_interval);

loop {
    tokio::select! {
        biased;
        () = cancel_token.cancelled() => break,
        _ = flush_timer.tick() => {
            if let Err(e) = sink.flush().await {
                tracing::warn!(error = %e, "Batch flush failed");
            }
        }
        envelope = receiver.recv() => {
            // existing collect logic
        }
    }
}
```

### Decision 6: Config Schema Extensions

**Chosen:** Add `OverflowPolicy` enum to config schema, add `overflow` field to `EdgeDefinition`, add `DeadLetterConfig` section.

**Changes:**
```rust
#[derive(Debug, Clone, Copy, Default, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum OverflowPolicy {
    #[default]
    Slow,
    Drop,
    DeadLetter,
}

#[derive(Debug, Clone, Deserialize)]
pub struct EdgeDefinition {
    pub from: String,
    pub to: String,
    #[serde(default)]
    pub from_port: Option<String>,
    #[serde(default)]
    pub to_port: Option<String>,
    #[serde(default)]
    pub queue_capacity: Option<usize>,
    #[serde(default)]  // NEW
    pub overflow: OverflowPolicy,
}

#[derive(Debug, Clone, Deserialize)]
pub struct DeadLetterConfig {
    #[serde(default)]
    pub enabled: bool,
    pub sink_type: Option<String>,
    #[serde(default)]
    pub config: toml::Value,
}
```

## Risks / Trade-offs

### Risk 1: DLQ Sink Failure

**Risk:** If the DLQ sink itself fails (e.g., disk full, MQTT broker down), messages are lost.

**Mitigation:**
- Log DLQ failures at ERROR level with full message context
- Add `dlq_sink_error_total` metric for alerting
- Document that DLQ is best-effort, not guaranteed delivery
- Future: Add optional "fallback to stderr" mode

### Risk 2: Batch Timeout Starvation

**Risk:** If `flush_interval` timer races with `collect()`, messages could be delayed.

**Mitigation:**
- Use `tokio::time::interval` with `MissedTickBehavior::Delay`
- Flush on both timeout AND batch size
- Ensure flush is called before `close()` in shutdown path

### Risk 3: Memory Growth with Large Batches

**Risk:** Large `batch_size` with slow downstream could accumulate memory.

**Mitigation:**
- Document recommended batch_size ranges (10-1000)
- Add `sink_batch_buffer_size` gauge metric
- Consider adding max_buffer_bytes limit (future)

### Risk 4: DLQ Serialization Overhead

**Risk:** Serializing `DlqEnvelope` to JSON on every DLQ message adds CPU overhead.

**Mitigation:**
- DLQ should be exceptional path, not hot path
- Use `serde_json::to_vec` (not pretty-print)
- Consider optional binary format (future)

### Risk 5: Backward Compatibility

**Risk:** Existing configs might break if new fields are required.

**Mitigation:**
- All new fields have sensible defaults
- `overflow` defaults to `Slow` (current behavior)
- `dead_letter.enabled` defaults to `false`
- `batch_size` defaults to `None` (no batching, immediate flush)

## Resolved Questions

1. **Should DLQ wrap the entire `DlqEnvelope` in a new `RuntimeEnvelope`?**
   - **Decision: Yes.** Serialize `DlqEnvelope` as JSON payload in a new `RuntimeEnvelope`.
   - **Rationale:** Consistent with pipeline message format, allows DLQ sink to use standard `collect()` interface.

2. **Should `dead-letter` policy require DLQ to be configured?**
   - **Decision: Yes.** Fail validation if `overflow = "dead-letter"` but no `[dead_letter]` section enabled.
   - **Rationale:** Fail-fast catches misconfiguration early; silent fallback to drop would hide bugs.

3. **Should batch timeout be absolute or sliding window?**
   - **Decision: Absolute.** Flush every N ms regardless of activity.
   - **Rationale:** Simpler implementation, predictable latency guarantee, consistent with industry practice (Kafka, etc.).

4. **Should we support per-sink DLQ or pipeline-wide only?**
   - **Decision: Pipeline-wide only for MVP.** Per-sink DLQ as future enhancement.
   - **Rationale:** Covers 90% of use cases, simpler to implement and reason about.

5. **What happens when DLQ sender is full?**
   - **Decision: DLQ sender uses blocking `send().await`.**
   - **Rationale:** DLQ should be sized for worst-case; if DLQ backs up, it's a signal the pipeline has bigger problems. Log warning when DLQ queue reaches 80% capacity.

## New Dependency

- **chrono**: Required for `DlqEnvelope.failed_at` timestamp field. Add `chrono = { version = "0.4", features = ["serde"] }` to `wafer-core/Cargo.toml`.
