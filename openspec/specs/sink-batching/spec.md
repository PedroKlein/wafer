## ADDED Requirements

### Requirement: Configurable batch size for sinks

The system SHALL support configuring a `batch_size` option for sink nodes that buffers messages before flushing.

#### Scenario: Batch size configuration
- **WHEN** a sink is configured with `batch_size = 100`
- **THEN** the sink SHALL buffer up to 100 messages before flushing to the destination

#### Scenario: Default no batching
- **WHEN** a sink is configured without `batch_size`
- **THEN** each message SHALL be written immediately (batch_size = 1)

#### Scenario: Batch size of 1
- **WHEN** a sink is configured with `batch_size = 1`
- **THEN** each message SHALL be written immediately (no batching)

### Requirement: Configurable batch timeout for sinks

The system SHALL support configuring a `batch_timeout_ms` option that triggers a flush after the specified duration, regardless of batch size.

#### Scenario: Timeout triggers flush
- **WHEN** `batch_timeout_ms = 500` AND 500ms passes since last flush AND buffer is non-empty
- **THEN** the sink SHALL flush all buffered messages

#### Scenario: Timeout with partial batch
- **WHEN** batch has 10 messages AND `batch_size = 100` AND timeout expires
- **THEN** the sink SHALL flush all 10 messages without waiting for full batch

#### Scenario: Default no timeout
- **WHEN** a sink is configured with `batch_size` but no `batch_timeout_ms`
- **THEN** the sink SHALL only flush when batch size is reached or on shutdown

#### Scenario: Timeout without batch size
- **WHEN** a sink is configured with `batch_timeout_ms` but no `batch_size`
- **THEN** messages SHALL be batched with a default batch_size of unlimited (flush only on timeout)

### Requirement: Flush on batch size reached

The system SHALL flush the batch immediately when the configured batch size is reached.

#### Scenario: Flush on size threshold
- **WHEN** the buffer reaches `batch_size` messages
- **THEN** the sink SHALL immediately flush all buffered messages

#### Scenario: Counter resets after flush
- **WHEN** a batch is flushed due to size threshold
- **THEN** the message counter SHALL reset to zero for the next batch

### Requirement: Sink trait flush method

The `Sink` trait SHALL include a `flush()` method for explicitly flushing buffered messages.

#### Scenario: Flush method signature
- **WHEN** implementing the `Sink` trait
- **THEN** the trait SHALL include `fn flush(&mut self) -> Pin<Box<dyn Future<Output = Result<()>> + Send + '_>>`

#### Scenario: Default flush implementation
- **WHEN** a sink does not override `flush()`
- **THEN** the default implementation SHALL return `Ok(())` (no-op for non-batching sinks)

#### Scenario: Custom flush implementation
- **WHEN** a batching sink implements `flush()`
- **THEN** it SHALL write all buffered messages to the destination

### Requirement: Flush before close

The system SHALL call `flush()` before `close()` during sink shutdown to ensure no buffered messages are lost.

#### Scenario: Shutdown flush sequence
- **WHEN** the pipeline is shutting down
- **THEN** the system SHALL call `sink.flush()` before `sink.close()`

#### Scenario: Graceful shutdown preserves messages
- **WHEN** shutdown is initiated AND buffer contains messages
- **THEN** all buffered messages SHALL be written before the sink closes

#### Scenario: Flush error during shutdown
- **WHEN** `flush()` returns an error during shutdown
- **THEN** the error SHALL be logged at WARN level AND `close()` SHALL still be called

### Requirement: Batch metrics

The system SHALL expose Prometheus metrics for sink batching operations.

#### Scenario: Flush counter metric
- **WHEN** a batch is flushed
- **THEN** the system SHALL increment `wafer_sink_batch_flush_total{sink="<sink_id>"}` counter

#### Scenario: Batch size histogram
- **WHEN** a batch is flushed
- **THEN** the system SHALL record the batch size in `wafer_sink_batch_size{sink="<sink_id>"}` histogram

#### Scenario: Buffer size gauge
- **WHEN** metrics are scraped
- **THEN** `wafer_sink_batch_buffer_size{sink="<sink_id>"}` gauge SHALL report current buffer occupancy

### Requirement: FileSink batching support

The `FileSink` implementation SHALL support batching by buffering messages and writing them as a single I/O operation.

#### Scenario: Batched file writes
- **WHEN** FileSink is configured with `batch_size = 50`
- **THEN** 50 messages SHALL be accumulated before writing to file

#### Scenario: FileSink flush writes all
- **WHEN** `flush()` is called on FileSink with 30 buffered messages
- **THEN** all 30 messages SHALL be written to the file

#### Scenario: FileSink newline separation
- **WHEN** batched messages are written to file
- **THEN** each message SHALL be separated by a newline character

### Requirement: MqttSink batching support

The `MqttSink` implementation SHALL support batching by buffering messages and publishing them in rapid succession.

#### Scenario: Batched MQTT publish
- **WHEN** MqttSink is configured with `batch_size = 20`
- **THEN** messages SHALL be buffered and published in a batch when threshold is reached

#### Scenario: MqttSink flush publishes all
- **WHEN** `flush()` is called on MqttSink with buffered messages
- **THEN** all buffered messages SHALL be published to the MQTT broker

#### Scenario: MqttSink QoS per message
- **WHEN** batched messages are published
- **THEN** each message SHALL retain its configured QoS level

### Requirement: StdoutSink batching support

The `StdoutSink` implementation SHALL support batching by buffering messages and writing them together.

#### Scenario: Batched stdout writes
- **WHEN** StdoutSink is configured with `batch_size = 10`
- **THEN** 10 messages SHALL be accumulated before writing to stdout

#### Scenario: StdoutSink flush writes all
- **WHEN** `flush()` is called on StdoutSink with buffered messages
- **THEN** all buffered messages SHALL be written to stdout

### Requirement: BatchBuffer utility

The system SHALL provide a `BatchBuffer<T>` utility struct for implementing batching in sinks.

#### Scenario: Push returns flush signal
- **WHEN** `batch_buffer.push(item)` is called AND batch size is reached
- **THEN** the method SHALL return `Some(Vec<T>)` containing all buffered items

#### Scenario: Push buffers when not full
- **WHEN** `batch_buffer.push(item)` is called AND batch size is not reached
- **THEN** the method SHALL return `None` and buffer the item

#### Scenario: Take drains buffer
- **WHEN** `batch_buffer.take()` is called
- **THEN** the method SHALL return all buffered items and reset the buffer

#### Scenario: Timeout check
- **WHEN** `batch_buffer.should_flush()` is called AND timeout has elapsed
- **THEN** the method SHALL return `true`
