# Pipeline Control Specification

Core control interface for WAFER pipeline operations. This trait is the ABI-stable surface for all control operations, whether accessed via HTTP API or direct library calls.

## ADDED Requirements

### Requirement: Hot-swap single node

The system SHALL provide an async method to trigger hot-swap on a specific node by ID. The method SHALL wait for the drain-and-flip process to complete before returning. Only one hot-swap operation MAY be in progress at a time.

#### Scenario: Successful hot-swap

- **WHEN** `hot_swap("filter")` is called while no other swap is in progress
- **THEN** the system drains the node, loads the new WASM, flips routing, and returns `HotSwapResult` with timing metrics

#### Scenario: Node not found

- **WHEN** `hot_swap("nonexistent")` is called
- **THEN** the system returns `ControlError::NodeNotFound` immediately

#### Scenario: Concurrent hot-swap rejected

- **WHEN** `hot_swap("node-a")` is called while another hot-swap is in progress
- **THEN** the system returns `ControlError::SwapInProgress` immediately

#### Scenario: Hot-swap on source/sink rejected

- **WHEN** `hot_swap("source")` is called on a native source or sink node
- **THEN** the system returns `ControlError::NotSwappable` (only WASM nodes support hot-swap)

#### Scenario: Hot-swap not implemented (stub)

- **WHEN** `hot_swap()` is called before hot-swap-mechanism is implemented
- **THEN** the system returns `ControlError::NotImplemented`
- **NOTE** This scenario is temporary during incremental implementation

### Requirement: Reload configuration

The system SHALL provide an async method to reload configuration from the config file, detect changed nodes, and trigger hot-swap for each changed node. The method SHALL return a list of nodes that were swapped.

#### Scenario: Config reload with changes

- **WHEN** `reload_config()` is called and the config file has changed node `filter`'s plugin_path
- **THEN** the system triggers hot-swap for `filter` and returns `ReloadResult { swapped_nodes: ["filter"] }`

#### Scenario: Config reload with no changes

- **WHEN** `reload_config()` is called and the config file is unchanged
- **THEN** the system returns `ReloadResult { swapped_nodes: [] }` without performing any swaps

#### Scenario: Config reload with invalid config

- **WHEN** `reload_config()` is called and the config file contains invalid TOML
- **THEN** the system returns `ControlError::ConfigError` with details

### Requirement: Drain pipeline

The system SHALL provide an async method to drain the pipeline (stop accepting new messages, finish in-flight work). Drain SHALL be idempotent.

#### Scenario: Successful drain

- **WHEN** `drain()` is called on a running pipeline
- **THEN** the system stops sources from producing new messages, allows in-flight messages to complete, and returns success

#### Scenario: Drain already draining

- **WHEN** `drain()` is called while pipeline is already draining
- **THEN** the system returns success immediately (idempotent)

### Requirement: Graceful shutdown

The system SHALL provide an async method to gracefully shut down the pipeline. Shutdown SHALL drain first, then close all nodes.

#### Scenario: Successful shutdown

- **WHEN** `shutdown()` is called
- **THEN** the system drains the pipeline, closes all nodes in reverse topological order, and returns success

### Requirement: Query pipeline status

The system SHALL provide a sync method to query current pipeline status. This method MUST be cheap (no blocking I/O).

#### Scenario: Query running pipeline

- **WHEN** `status()` is called on a running pipeline
- **THEN** the system returns `PipelineStatus { name, state: Running, uptime, ... }`

#### Scenario: Query draining pipeline

- **WHEN** `status()` is called while pipeline is draining
- **THEN** the system returns `PipelineStatus { state: Draining, ... }`

### Requirement: Query metrics snapshot

The system SHALL provide a sync method to get a snapshot of current metrics. This method MUST be cheap (no blocking I/O).

#### Scenario: Get metrics

- **WHEN** `metrics()` is called
- **THEN** the system returns `MetricsSnapshot` with message counts, latencies, queue depths, etc.

### Requirement: List nodes

The system SHALL provide a sync method to list all nodes with their current state. This method MUST be cheap (no blocking I/O).

#### Scenario: List nodes

- **WHEN** `nodes()` is called
- **THEN** the system returns `Vec<NodeInfo>` with each node's ID, type, state, and metrics

### Requirement: Subscribe to events

The system SHALL provide a method to subscribe to pipeline events. Subscribers receive events via a broadcast channel.

#### Scenario: Subscribe and receive hot-swap events

- **WHEN** a client calls `subscribe()` and then hot-swap completes
- **THEN** the client receives `PipelineEvent::HotSwapStarted` followed by `PipelineEvent::HotSwapCompleted`

#### Scenario: Multiple subscribers

- **WHEN** two clients subscribe and an event occurs
- **THEN** both clients receive the event independently

#### Scenario: Slow subscriber

- **WHEN** a subscriber is slow to process events
- **THEN** the system MAY drop events for that subscriber (bounded buffer) without affecting other subscribers
