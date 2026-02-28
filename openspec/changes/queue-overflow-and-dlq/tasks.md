## 1. Config Schema Extensions

- [ ] 1.0 Add `chrono = { version = "0.4", features = ["serde"] }` dependency to `wafer-core/Cargo.toml`
- [ ] 1.1 Add `OverflowPolicy` enum to `crates/wafer-core/src/config/schema.rs` with `Slow`, `Drop`, `DeadLetter` variants and serde kebab-case deserialization
- [ ] 1.2 Add `overflow: OverflowPolicy` field to `EdgeDefinition` with `#[serde(default)]`
- [ ] 1.3 Add `DeadLetterConfig` struct with `enabled`, `sink_type`, `config`, and `queue_capacity` (default 10,000) fields
- [ ] 1.4 Add `dead_letter: Option<DeadLetterConfig>` field to `Config` struct
- [ ] 1.5 Add validation in `DagConfig::validate()` to ensure DLQ is configured when any edge uses `dead-letter` policy
- [ ] 1.6 Write unit tests for config parsing: valid overflow values, invalid overflow rejected, DLQ validation

## 2. DLQ Envelope Types

- [ ] 2.1 Create `crates/wafer-core/src/dlq/mod.rs` module with `DlqReason` enum (`QueueFull`, `ProcessError`, `SinkError`)
- [ ] 2.2 Add `DlqEnvelope` struct with `original`, `failed_edge`, `reason`, `failed_at` fields
- [ ] 2.3 Implement `Serialize`/`Deserialize` for `DlqEnvelope` with base64 encoding for binary payloads
- [ ] 2.4 Add `wrap_for_dlq()` helper function to create `DlqEnvelope` from `RuntimeEnvelope` and failure context
- [ ] 2.5 Write unit tests for DLQ envelope serialization roundtrip

## 3. DLQ Sink Infrastructure

- [ ] 3.1 Add `DlqSink` wrapper struct that holds a `Box<dyn Sink>` and handles DLQ envelope serialization
- [ ] 3.2 Implement factory function `create_dlq_sink(config: &DeadLetterConfig)` that returns appropriate sink type
- [ ] 3.3 Add `dlq_sender: Option<QueueSender<RuntimeEnvelope>>` to `ControlState` in orchestrator (DLQ receives RuntimeEnvelope with JSON-serialized DlqEnvelope as payload)
- [ ] 3.4 Initialize DLQ sink and spawn DLQ processing task in `DagOrchestrator::new()` when enabled
- [ ] 3.5 Write integration test for DLQ sink initialization with file sink type

## 4. Queue Overflow Policy Implementation

- [ ] 4.1 Add `overflow_policy: OverflowPolicy` field to edge metadata in DAG builder
- [ ] 4.2 Thread overflow policy and DLQ sender through `run_node_loop()` parameters
- [ ] 4.3 Implement overflow handling in source loop: use `try_send()` for drop/dead-letter policies
- [ ] 4.4 Implement overflow handling in transform loop: route to DLQ on queue full with dead-letter policy
- [ ] 4.5 Implement overflow handling in router loop: route to DLQ on queue full with dead-letter policy
- [ ] 4.6 Implement overflow handling in joiner loop: route to DLQ on queue full with dead-letter policy
- [ ] 4.7 Write unit tests for each overflow policy behavior in isolation

## 5. Process Error DLQ Routing

- [ ] 5.1 Modify transform loop to route `ProcessResult::Error` messages to DLQ when enabled
- [ ] 5.2 Modify router loop to route `RouteResult::Error` messages to DLQ when enabled
- [ ] 5.3 Modify joiner loop to route `ProcessResult::Error` messages to DLQ when enabled
- [ ] 5.4 Modify sink loop to route failed messages to DLQ when `collect()` returns error
- [ ] 5.5 Ensure DLQ routing preserves original message and captures error details
- [ ] 5.6 Add DLQ queue capacity warning when reaching 80% (log warning)
- [ ] 5.7 Write integration test for error-to-DLQ routing path

## 6. Overflow Metrics

- [ ] 6.1 Add `queue_drop_total` counter to metrics registry with `edge` label
- [ ] 6.2 Add `queue_dlq_total` counter to metrics registry with `edge` label
- [ ] 6.3 Add `dlq_sink_error_total` counter to metrics registry
- [ ] 6.4 Instrument overflow handling code to increment appropriate counters
- [ ] 6.5 Write test to verify metrics are incremented on drop/DLQ events

## 7. BatchBuffer Utility

- [ ] 7.1 Create `crates/wafer-core/src/node/sink/batch.rs` with `BatchBuffer<T>` struct
- [ ] 7.2 Implement `push()` method that returns `Some(Vec<T>)` when batch size reached
- [ ] 7.3 Implement `take()` method to drain buffer
- [ ] 7.4 Implement `should_flush()` method for timeout checking
- [ ] 7.5 Add `last_flush: Instant` tracking for timeout calculations
- [ ] 7.6 Write unit tests for BatchBuffer push/take/timeout behavior

## 8. Sink Trait Extension

- [ ] 8.1 Add `flush()` method to `Sink` trait with default no-op implementation
- [ ] 8.2 Add `batch_timeout()` method to `Sink` trait returning `Option<Duration>`
- [ ] 8.3 Update sink loop to include flush timer in `tokio::select!`
- [ ] 8.4 Call `sink.flush()` before `sink.close()` in shutdown path
- [ ] 8.5 Write test to verify flush-before-close sequence

## 9. FileSink Batching

- [ ] 9.1 Add `batch_size` and `batch_timeout_ms` config fields to FileSink
- [ ] 9.2 Add `BatchBuffer<RuntimeEnvelope>` field to FileSink
- [ ] 9.3 Modify `collect()` to buffer messages and flush on batch size
- [ ] 9.4 Implement `flush()` to write all buffered messages with newline separation
- [ ] 9.5 Write integration test for FileSink batched writes

## 10. MqttSink Batching

- [ ] 10.1 Add `batch_size` and `batch_timeout_ms` config fields to MqttSink
- [ ] 10.2 Add `BatchBuffer<RuntimeEnvelope>` field to MqttSink
- [ ] 10.3 Modify `collect()` to buffer messages and publish on batch size
- [ ] 10.4 Implement `flush()` to publish all buffered messages
- [ ] 10.5 Write unit test for MqttSink batching (mock broker)

## 11. StdoutSink Batching

- [ ] 11.1 Add `batch_size` and `batch_timeout_ms` config fields to StdoutSink
- [ ] 11.2 Add `BatchBuffer<RuntimeEnvelope>` field to StdoutSink
- [ ] 11.3 Modify `collect()` to buffer messages and write on batch size
- [ ] 11.4 Implement `flush()` to write all buffered messages
- [ ] 11.5 Write unit test for StdoutSink batching

## 12. Batching Metrics

- [ ] 12.1 Add `sink_batch_flush_total` counter with `sink` label
- [ ] 12.2 Add `sink_batch_size` histogram with `sink` label
- [ ] 12.3 Add `sink_batch_buffer_size` gauge with `sink` label
- [ ] 12.4 Instrument sink flush operations to record metrics
- [ ] 12.5 Write test to verify batching metrics are recorded

## 13. Integration Testing

- [ ] 13.1 Create end-to-end test with `drop` policy: verify messages dropped under load
- [ ] 13.2 Create end-to-end test with `dead-letter` policy: verify failed messages appear in DLQ file
- [ ] 13.3 Create end-to-end test with sink batching: verify batch flush timing
- [ ] 13.4 Create test combining overflow + batching: verify both work together
- [ ] 13.5 Verify backward compatibility: existing configs without new fields still work

## 14. Documentation

- [ ] 14.1 Update docs/SPEC.md §8.2 to mark overflow policies as implemented
- [ ] 14.2 Update docs/SPEC.md §7.2 to mark DLQ as implemented
- [ ] 14.3 Update docs/SPEC.md §4.9 to mark sink batching as implemented
- [ ] 14.4 Update docs/MVP.md to reflect new capabilities
- [ ] 14.5 Add example TOML configuration showing all new options
