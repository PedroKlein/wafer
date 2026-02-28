# Structured Logging

JSON-formatted log output for log aggregation and analysis.

**SPEC Reference:** Section 12.4

## ADDED Requirements

### Requirement: JSON log format

The runtime SHALL output logs in structured JSON format to stdout.

#### Scenario: Standard log message
- **WHEN** a log event is emitted at INFO level
- **THEN** output is valid JSON
- **THEN** JSON includes `timestamp`, `level`, `target`, `message` fields
- **THEN** timestamp is in RFC 3339 format with nanosecond precision

#### Scenario: Log with span context
- **WHEN** a log event is emitted within a tracing span
- **THEN** JSON includes `span` object with span fields (e.g., `pipeline`, `node_id`)

#### Scenario: Log with additional fields
- **WHEN** a log event includes structured fields
- **THEN** JSON includes `fields` object with key-value pairs

### Requirement: Log levels

The runtime SHALL support standard log levels configurable via environment variable.

#### Scenario: Default log level
- **WHEN** `RUST_LOG` environment variable is not set
- **THEN** log level defaults to INFO

#### Scenario: Custom log level
- **WHEN** `RUST_LOG=debug` environment variable is set
- **THEN** DEBUG and higher level messages are logged

#### Scenario: Per-module log level
- **WHEN** `RUST_LOG=wafer=debug,hyper=warn` is set
- **THEN** wafer modules log at DEBUG level
- **THEN** hyper modules log at WARN level

### Requirement: Log structure compliance

Log output SHALL match the structure defined in SPEC §12.4.

#### Scenario: Message processing log
- **WHEN** a message is processed by a node
- **THEN** log includes span with `pipeline` and `node_id`
- **THEN** log includes fields: `message_id`, `duration_ns`, `input_size_bytes`, `output_size_bytes`
