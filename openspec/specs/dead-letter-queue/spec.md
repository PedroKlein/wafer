## ADDED Requirements

### Requirement: Pipeline-wide DLQ configuration

The system SHALL support a `[dead_letter]` configuration section for pipeline-wide Dead Letter Queue settings.

#### Scenario: DLQ disabled by default
- **WHEN** no `[dead_letter]` section is present in configuration
- **THEN** the system SHALL treat DLQ as disabled

#### Scenario: Explicit DLQ enable
- **WHEN** configuration contains `[dead_letter]` with `enabled = true`
- **THEN** the system SHALL initialize the DLQ sink at pipeline startup

#### Scenario: Explicit DLQ disable
- **WHEN** configuration contains `[dead_letter]` with `enabled = false`
- **THEN** the system SHALL NOT initialize any DLQ sink

### Requirement: DLQ sink type configuration

The system SHALL allow configuring the DLQ sink type using the `sink_type` field, supporting any valid sink type (`file`, `mqtt`, `stdout`).

#### Scenario: File sink DLQ
- **WHEN** DLQ is configured with `sink_type = "file"`
- **THEN** the system SHALL write DLQ messages to the configured file path

#### Scenario: MQTT sink DLQ
- **WHEN** DLQ is configured with `sink_type = "mqtt"`
- **THEN** the system SHALL publish DLQ messages to the configured MQTT topic

#### Scenario: Stdout sink DLQ
- **WHEN** DLQ is configured with `sink_type = "stdout"`
- **THEN** the system SHALL write DLQ messages to standard output

#### Scenario: Invalid sink type rejected
- **WHEN** DLQ is configured with an invalid `sink_type`
- **THEN** the system SHALL return a configuration validation error

### Requirement: DLQ envelope structure

The system SHALL wrap failed messages in a `DlqEnvelope` structure containing the original message and failure context.

#### Scenario: DlqEnvelope contains original message
- **WHEN** a message is sent to DLQ
- **THEN** the `DlqEnvelope.original` field SHALL contain the complete `RuntimeEnvelope`

#### Scenario: DlqEnvelope contains failed edge
- **WHEN** a message is sent to DLQ
- **THEN** the `DlqEnvelope.failed_edge` field SHALL contain the edge identifier in `"from->to"` format

#### Scenario: DlqEnvelope contains failure reason
- **WHEN** a message fails due to queue overflow
- **THEN** the `DlqEnvelope.reason` field SHALL be `QueueFull`

#### Scenario: DlqEnvelope contains process error reason
- **WHEN** a message fails due to transform/router processing error
- **THEN** the `DlqEnvelope.reason` field SHALL be `ProcessError` with code and message

#### Scenario: DlqEnvelope contains timestamp
- **WHEN** a message is sent to DLQ
- **THEN** the `DlqEnvelope.failed_at` field SHALL contain the UTC timestamp of failure

### Requirement: DLQ message serialization

The system SHALL wrap `DlqEnvelope` in a `RuntimeEnvelope` with JSON-serialized payload before writing to the DLQ sink.

#### Scenario: DLQ wraps in RuntimeEnvelope
- **WHEN** a message is routed to DLQ
- **THEN** the DLQ sink SHALL receive a `RuntimeEnvelope` with `DlqEnvelope` serialized as JSON payload

#### Scenario: JSON serialization format
- **WHEN** a message is written to DLQ
- **THEN** the payload SHALL be valid JSON representing the `DlqEnvelope` structure

#### Scenario: Serialization includes all fields
- **WHEN** a `DlqEnvelope` is serialized
- **THEN** the JSON SHALL include `original`, `failed_edge`, `reason`, and `failed_at` fields

#### Scenario: Original envelope payload preserved
- **WHEN** the original message payload is binary
- **THEN** the `original.payload` field SHALL be base64-encoded in the JSON output

### Requirement: DLQ sink error handling

The system SHALL handle DLQ sink failures gracefully without crashing the pipeline.

#### Scenario: DLQ sink write failure
- **WHEN** the DLQ sink fails to write a message (e.g., disk full, network error)
- **THEN** the system SHALL log the error at ERROR level and continue processing

#### Scenario: DLQ sink error metrics
- **WHEN** a DLQ sink write fails
- **THEN** the system SHALL increment `wafer_dlq_sink_error_total` counter

#### Scenario: Pipeline continues on DLQ failure
- **WHEN** the DLQ sink experiences persistent failures
- **THEN** the main pipeline processing SHALL continue unaffected

### Requirement: DLQ configuration validation

The system SHALL validate DLQ configuration at startup.

#### Scenario: Missing sink_type when enabled
- **WHEN** `dead_letter.enabled = true` AND `sink_type` is not specified
- **THEN** the system SHALL return a configuration validation error

#### Scenario: Missing config section
- **WHEN** DLQ requires configuration (e.g., file sink needs path) AND `[dead_letter.config]` is missing required fields
- **THEN** the system SHALL return a configuration validation error

#### Scenario: Valid minimal configuration
- **WHEN** DLQ is configured with `enabled = true`, `sink_type = "stdout"`
- **THEN** configuration validation SHALL succeed

### Requirement: DLQ sources for messages

The system SHALL route messages to DLQ from multiple failure sources: queue overflow (dead-letter policy), transform errors, router errors, and sink errors.

#### Scenario: Transform error routes to DLQ
- **WHEN** a transform returns `ProcessResult::Error` AND DLQ is enabled
- **THEN** the failed message SHALL be routed to DLQ with reason `ProcessError`

#### Scenario: Router error routes to DLQ
- **WHEN** a router returns `RouteResult::Error` AND DLQ is enabled
- **THEN** the failed message SHALL be routed to DLQ with reason `ProcessError`

#### Scenario: Sink error routes to DLQ
- **WHEN** a sink's `collect()` returns an error AND DLQ is enabled
- **THEN** the failed message SHALL be routed to DLQ with reason `SinkError`

#### Scenario: Joiner error routes to DLQ
- **WHEN** a joiner returns `ProcessResult::Error` AND DLQ is enabled
- **THEN** the failed message SHALL be routed to DLQ with reason `ProcessError`

#### Scenario: Errors without DLQ enabled
- **WHEN** a processing error occurs AND DLQ is disabled
- **THEN** the error SHALL be logged but no DLQ routing occurs

### Requirement: DLQ internal queue backpressure

The DLQ internal queue SHALL use blocking backpressure to ensure no DLQ messages are silently dropped.

#### Scenario: DLQ queue blocks when full
- **WHEN** the DLQ internal queue is full AND a new message is routed to DLQ
- **THEN** the sender SHALL block until space is available

#### Scenario: DLQ queue capacity warning
- **WHEN** the DLQ internal queue reaches 80% capacity
- **THEN** the system SHALL log a warning indicating DLQ backpressure

#### Scenario: DLQ queue sized appropriately
- **WHEN** DLQ is initialized
- **THEN** the DLQ queue capacity SHALL default to 10,000 messages (configurable via `dead_letter.queue_capacity`)
