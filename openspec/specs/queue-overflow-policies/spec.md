## ADDED Requirements

### Requirement: Configurable overflow policy per edge

The system SHALL support configuring an overflow policy for each edge in the DAG. The `overflow` field in `EdgeDefinition` SHALL accept one of three values: `slow` (default), `drop`, or `dead-letter`.

#### Scenario: Default overflow policy
- **WHEN** an edge is defined without an `overflow` field
- **THEN** the system SHALL use `slow` as the default overflow policy

#### Scenario: Explicit slow policy
- **WHEN** an edge is configured with `overflow = "slow"`
- **THEN** the system SHALL block the sender asynchronously when the queue is full until space becomes available

#### Scenario: Explicit drop policy
- **WHEN** an edge is configured with `overflow = "drop"`
- **THEN** the system SHALL accept the configuration and apply drop behavior when the queue is full

#### Scenario: Explicit dead-letter policy
- **WHEN** an edge is configured with `overflow = "dead-letter"`
- **THEN** the system SHALL accept the configuration and route dropped messages to the DLQ when the queue is full

### Requirement: Drop policy discards newest message

When an edge is configured with `overflow = "drop"`, the system SHALL discard the newest message (the one being sent) when the queue is full, rather than blocking.

#### Scenario: Drop on full queue
- **WHEN** the queue is full AND overflow policy is `drop` AND a new message arrives
- **THEN** the system SHALL discard the new message without blocking the sender

#### Scenario: Drop does not affect existing messages
- **WHEN** a message is dropped due to full queue
- **THEN** messages already in the queue SHALL remain unaffected and continue processing

#### Scenario: High-throughput drop handling
- **WHEN** messages arrive faster than they can be processed AND overflow policy is `drop`
- **THEN** the system SHALL continue to accept and drop messages without degrading processing of existing queue contents

### Requirement: Dead-letter policy routes to DLQ

When an edge is configured with `overflow = "dead-letter"`, the system SHALL route dropped messages to the configured Dead Letter Queue sink instead of discarding them.

#### Scenario: Route to DLQ on full queue
- **WHEN** the queue is full AND overflow policy is `dead-letter` AND a new message arrives
- **THEN** the system SHALL send the message to the DLQ sink with error context

#### Scenario: DLQ envelope contains original message
- **WHEN** a message is routed to DLQ due to queue overflow
- **THEN** the DLQ envelope SHALL contain the complete original `RuntimeEnvelope`

#### Scenario: DLQ envelope contains failure context
- **WHEN** a message is routed to DLQ
- **THEN** the DLQ envelope SHALL include: failed edge identifier, reason (`QueueFull`), and UTC timestamp

### Requirement: Dead-letter policy requires DLQ configuration

The system SHALL fail configuration validation if any edge uses `overflow = "dead-letter"` but no `[dead_letter]` section is configured or `dead_letter.enabled = false`.

#### Scenario: Validation fails without DLQ config
- **WHEN** configuration contains `overflow = "dead-letter"` on any edge AND no `[dead_letter]` section exists
- **THEN** the system SHALL return a configuration validation error

#### Scenario: Validation fails with disabled DLQ
- **WHEN** configuration contains `overflow = "dead-letter"` on any edge AND `dead_letter.enabled = false`
- **THEN** the system SHALL return a configuration validation error

#### Scenario: Validation passes with enabled DLQ
- **WHEN** configuration contains `overflow = "dead-letter"` AND `[dead_letter]` section exists with `enabled = true`
- **THEN** configuration validation SHALL succeed

### Requirement: Overflow metrics per edge

The system SHALL expose Prometheus metrics for overflow events on each edge.

#### Scenario: Drop counter metric
- **WHEN** a message is dropped due to `drop` policy
- **THEN** the system SHALL increment `wafer_queue_drop_total{edge="<from>-><to>"}` counter

#### Scenario: DLQ counter metric
- **WHEN** a message is routed to DLQ due to `dead-letter` policy
- **THEN** the system SHALL increment `wafer_queue_dlq_total{edge="<from>-><to>"}` counter

#### Scenario: Metrics are zero by default
- **WHEN** the pipeline starts AND no overflow events have occurred
- **THEN** drop and DLQ counters SHALL be initialized to zero for all edges

### Requirement: OverflowPolicy enum in config schema

The configuration schema SHALL include an `OverflowPolicy` enum with variants `Slow`, `Drop`, and `DeadLetter`.

#### Scenario: Parse slow from TOML
- **WHEN** TOML contains `overflow = "slow"`
- **THEN** the system SHALL deserialize to `OverflowPolicy::Slow`

#### Scenario: Parse drop from TOML
- **WHEN** TOML contains `overflow = "drop"`
- **THEN** the system SHALL deserialize to `OverflowPolicy::Drop`

#### Scenario: Parse dead-letter from TOML
- **WHEN** TOML contains `overflow = "dead-letter"`
- **THEN** the system SHALL deserialize to `OverflowPolicy::DeadLetter`

#### Scenario: Reject invalid overflow value
- **WHEN** TOML contains `overflow = "invalid"`
- **THEN** the system SHALL return a deserialization error
