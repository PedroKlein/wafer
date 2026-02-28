# Hot-Swap Metrics

Timing and operational metrics for monitoring hot-swap performance.

**SPEC Reference:** Section 10.2

## ADDED Requirements

### Requirement: Phase timing metrics

The runtime SHALL collect timing metrics for each phase of the hot-swap operation.

#### Scenario: Successful swap timing
- **WHEN** a hot-swap completes successfully
- **THEN** `swap_prepare_time_ns` records time to load and initialize the new component
- **THEN** `swap_drain_time_ns` records time waiting for the old node to drain
- **THEN** `swap_flip_time_ns` records time for the atomic swap operation
- **THEN** `swap_retire_time_ns` records time to close and free the old node
- **THEN** `swap_total_time_ns` records the sum of all phases

#### Scenario: Failed swap timing
- **WHEN** a hot-swap fails during prepare phase
- **THEN** `swap_prepare_time_ns` records the time until failure
- **THEN** `swap_total_time_ns` equals `swap_prepare_time_ns`
- **THEN** other phase metrics are not recorded

### Requirement: Message tracking metrics

The runtime SHALL track message counts during hot-swap operations.

#### Scenario: Messages in flight at drain start
- **WHEN** drain phase begins
- **THEN** `swap_messages_in_flight` records the number of messages in the node's input queue

#### Scenario: Messages delayed during swap
- **WHEN** messages arrive during drain/flip phases
- **THEN** `swap_messages_delayed` records the count of messages that waited

#### Scenario: Messages dropped on timeout
- **WHEN** drain timeout forces the swap
- **THEN** `swap_messages_dropped` records the count of messages that were discarded

### Requirement: Swap outcome metrics

The runtime SHALL track hot-swap success and failure counts.

#### Scenario: Successful swap count
- **WHEN** a hot-swap completes successfully
- **THEN** `swap_success_total` counter is incremented
- **THEN** the counter is labeled with `node_id`

#### Scenario: Failed swap count
- **WHEN** a hot-swap fails (validation, init, or other error)
- **THEN** `swap_failure_total` counter is incremented
- **THEN** the counter is labeled with `node_id` and `reason`

### Requirement: Metrics export format

All hot-swap metrics SHALL be exportable in Prometheus format.

#### Scenario: Prometheus scrape
- **WHEN** the metrics endpoint is scraped
- **THEN** all hot-swap metrics are included in Prometheus text format
- **THEN** metrics include appropriate labels (`node_id`, `pipeline_id`)
