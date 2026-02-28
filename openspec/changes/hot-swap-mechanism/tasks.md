# Hot-Swap Implementation Tasks

> **Dependencies:** This change depends on `runtime-control-plane` for REST API triggers.
> Implement core hot-swap logic here; triggers are wired in runtime-control-plane.

## 1. Node State Machine

- [x] 1.1 Add `NodeState` enum to `crates/wafer-core/src/node/mod.rs` (Starting, Running, Draining, Retired)
- [x] 1.2 Add state field and accessor to `AnyNode` enum
- [x] 1.3 Add `processing` AtomicBool flag to WASM transform wrapper
- [x] 1.4 Update node execution loop to set processing flag around `process()` calls
- [x] 1.5 Write unit tests for state transitions

## 2. Message Routing Control

- [x] 2.1 Add `routing_enabled` AtomicBool to node wrapper structs
- [x] 2.2 Modify send logic in DAG runner to check routing_enabled before enqueue
- [x] 2.3 Add buffering for messages when routing is disabled (Vec<RuntimeEnvelope>)
- [x] 2.4 Add flush logic to send buffered messages when routing re-enabled
- [x] 2.5 Write integration test: pause routing, queue messages, resume, verify delivery

## 3. Hot-Swap Coordinator Core

- [x] 3.1 Add `HotSwapCoordinator` struct to `src/dag/mod.rs`
- [x] 3.2 Implement `prepare()`: load component, instantiate, call validate/init
- [x] 3.3 Implement `drain()`: set draining state, disable routing, wait for empty queue
- [x] 3.4 Implement `flip()`: swap node reference in orchestrator
- [x] 3.5 Implement `retire()`: call close(), drop old instance
- [x] 3.6 Add `hot_swap()` method to `DagOrchestrator` that sequences all phases
- [x] 3.7 Write integration test: swap transform node v1→v2, verify messages processed

## 4. Drain Timeout Handling

- [x] 4.1 Add drain timeout parameter to hot_swap() method
- [x] 4.2 Implement tokio::time::timeout wrapper around drain wait
- [x] 4.3 On timeout: force flip, count dropped messages from old queue
- [x] 4.4 Log dropped message count at WARN level
- [x] 4.5 Write test: simulate slow drain, verify timeout behavior

## 5. Config Diff & Resync

> **Note:** Trigger mechanisms (REST API, CLI) are in the `runtime-control-plane` change.
> This section covers the config diffing logic that resync uses.

- [x] 5.1 Create `src/config/diff.rs` with ConfigDiff struct
- [x] 5.2 Implement config comparison (detect WASM path changes)
- [x] 5.3 Add `resync()` method to DagOrchestrator that diffs and swaps
- [x] 5.4 Write unit test: config diff detects WASM path change
- [x] 5.5 Write integration test: resync triggers hot-swap for changed node

## 6. Swap Validation & Safety

- [x] 6.1 Validate new WASM component before starting drain (fail fast)
- [x] 6.2 Add swap-in-progress lock per node (prevent concurrent swaps)
- [x] 6.3 Implement abort path: on validation failure, log error, keep old node
- [x] 6.4 Write test: trigger swap with invalid WASM, verify old node continues

## 7. Hot-Swap Metrics

- [x] 7.1 Add swap timing fields to `PipelineMetrics` struct
- [x] 7.2 Instrument prepare phase with timing capture
- [x] 7.3 Instrument drain phase with timing capture
- [x] 7.4 Instrument flip phase with timing capture
- [x] 7.5 Instrument retire phase with timing capture
- [x] 7.6 Add message count metrics (in_flight, delayed, dropped)
- [x] 7.7 Add success/failure counters with labels
- [x] 7.8 Write test: perform swap, verify metrics populated

## 8. Documentation & Examples

- [x] 8.1 Update MVP.md with hot-swap status
- [x] 8.2 Create example config showing hot-swap usage
- [x] 8.3 Add hot-swap section to REGISTRY.md (swapping to new OCI version)
- [x] 8.4 Update SPEC.md milestone checkboxes

## 9. Benchmarking

- [x] 9.1 Create benchmark harness for hot-swap timing
- [x] 9.2 Measure swap latency under idle conditions
- [x] 9.3 Measure swap latency under load (100 msg/s, 1000 msg/s)
- [x] 9.4 Measure message loss under timeout conditions
- [x] 9.5 Document results and compare against SPEC targets (< 100ms)
