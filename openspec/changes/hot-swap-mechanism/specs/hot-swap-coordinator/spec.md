# Hot-Swap Coordinator

Core drain-and-flip algorithm implementation for replacing WASM nodes at runtime without stopping the pipeline.

**SPEC Reference:** Section 10.1-10.3, ADR-0003

## ADDED Requirements

### Requirement: Drain-and-flip algorithm

The hot-swap coordinator SHALL implement the drain-and-flip algorithm with four phases:
1. **PREPARE**: Load new component, instantiate in Wasmtime, call `validate()` and `init()`
2. **DRAIN**: Mark old node as draining, stop routing new messages, wait for in-flight completion
3. **FLIP**: Atomically swap routing to new node
4. **RETIRE**: Call `close()` on old node, drop instance, free resources

#### Scenario: Successful hot-swap of transform node
- **WHEN** hot-swap is triggered for a running transform node with a valid new WASM component
- **THEN** the new component is loaded and initialized
- **THEN** the old node stops receiving new messages
- **THEN** in-flight messages complete processing
- **THEN** routing switches atomically to the new node
- **THEN** the old node is closed and freed
- **THEN** no messages are lost or duplicated

#### Scenario: Hot-swap with messages in flight
- **WHEN** hot-swap is triggered while the node has messages in its input queue
- **THEN** all queued messages are processed by the old node before flip
- **THEN** new messages arriving during drain are held until flip completes
- **THEN** held messages are delivered to the new node after flip

### Requirement: Node state machine

Each WASM node SHALL maintain an explicit lifecycle state:
- `Starting`: Node is being initialized
- `Running`: Normal operation, processing messages
- `Draining`: No new messages accepted, processing remaining in-flight
- `Retired`: Node closed, awaiting cleanup

#### Scenario: State transitions during normal operation
- **WHEN** a node is created and initialized successfully
- **THEN** the node state transitions from `Starting` to `Running`

#### Scenario: State transitions during hot-swap
- **WHEN** hot-swap drain phase begins
- **THEN** the node state transitions from `Running` to `Draining`
- **WHEN** drain completes and flip occurs
- **THEN** the old node state transitions from `Draining` to `Retired`

### Requirement: Validation failure aborts swap

The coordinator SHALL abort the hot-swap if the new component fails validation or initialization, leaving the old node running unchanged.

#### Scenario: New component fails validate()
- **WHEN** hot-swap is triggered with a WASM component that returns error from `validate()`
- **THEN** the hot-swap is aborted
- **THEN** the old node continues running unchanged
- **THEN** an error is logged with the validation failure reason

#### Scenario: New component fails init()
- **WHEN** hot-swap is triggered with a WASM component that returns error from `init()`
- **THEN** the hot-swap is aborted
- **THEN** the old node continues running unchanged
- **THEN** an error is logged with the initialization failure reason

### Requirement: Drain timeout handling

The coordinator SHALL enforce a configurable drain timeout (`drain_timeout_ms`). If the timeout expires before drain completes, the swap SHALL be forced.

#### Scenario: Drain completes within timeout
- **WHEN** hot-swap drain phase completes before `drain_timeout_ms` expires
- **THEN** the swap proceeds normally with zero message loss

#### Scenario: Drain timeout expires
- **WHEN** drain phase does not complete within `drain_timeout_ms`
- **THEN** the swap is forced (flip occurs immediately)
- **THEN** messages still in the old node's queue are dropped
- **THEN** the count of dropped messages is logged
- **THEN** the `swap_messages_dropped` metric is incremented

### Requirement: Message routing control

The coordinator SHALL be able to pause and resume message routing to individual nodes during the drain phase.

#### Scenario: Pause routing during drain
- **WHEN** drain phase begins for a node
- **THEN** upstream nodes stop sending new messages to that node's input queue
- **THEN** messages are buffered at the sender until routing resumes

#### Scenario: Resume routing after flip
- **WHEN** flip phase completes
- **THEN** routing is enabled to the new node
- **THEN** buffered messages are delivered to the new node
