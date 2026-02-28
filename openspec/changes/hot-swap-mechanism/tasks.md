# Hot-Swap Implementation Tasks

> **Dependencies:** This change depends on `runtime-control-plane` for REST API triggers.
> Implement core hot-swap logic here; triggers are wired in runtime-control-plane.

## 1. Node State Machine

- [ ] 1.1 Add `NodeState` enum to `crates/wafer-core/src/node/mod.rs` (Starting, Running, Draining, Retired)
- [ ] 1.2 Add state field and accessor to `AnyNode` enum
- [ ] 1.3 Add `processing` AtomicBool flag to WASM transform wrapper
- [ ] 1.4 Update node execution loop to set processing flag around `process()` calls
- [ ] 1.5 Write unit tests for state transitions

## 2. Message Routing Control

- [ ] 2.1 Add `routing_enabled` AtomicBool to node wrapper structs
- [ ] 2.2 Modify send logic in DAG runner to check routing_enabled before enqueue
- [ ] 2.3 Add buffering for messages when routing is disabled (Vec<RuntimeEnvelope>)
- [ ] 2.4 Add flush logic to send buffered messages when routing re-enabled
- [ ] 2.5 Write integration test: pause routing, queue messages, resume, verify delivery

## 3. Hot-Swap Coordinator Core

- [ ] 3.1 Add `HotSwapCoordinator` struct to `src/dag/mod.rs`
- [ ] 3.2 Implement `prepare()`: load component, instantiate, call validate/init
- [ ] 3.3 Implement `drain()`: set draining state, disable routing, wait for empty queue
- [ ] 3.4 Implement `flip()`: swap node reference in orchestrator
- [ ] 3.5 Implement `retire()`: call close(), drop old instance
- [ ] 3.6 Add `hot_swap()` method to `DagOrchestrator` that sequences all phases
- [ ] 3.7 Write integration test: swap transform node v1→v2, verify messages processed

## 4. Drain Timeout Handling

- [ ] 4.1 Add drain timeout parameter to hot_swap() method
- [ ] 4.2 Implement tokio::time::timeout wrapper around drain wait
- [ ] 4.3 On timeout: force flip, count dropped messages from old queue
- [ ] 4.4 Log dropped message count at WARN level
- [ ] 4.5 Write test: simulate slow drain, verify timeout behavior

## 5. Config Diff & Resync

> **Note:** Trigger mechanisms (REST API, CLI) are in the `runtime-control-plane` change.
> This section covers the config diffing logic that resync uses.

- [ ] 5.1 Create `src/config/diff.rs` with ConfigDiff struct
- [ ] 5.2 Implement config comparison (detect WASM path changes)
- [ ] 5.3 Add `resync()` method to DagOrchestrator that diffs and swaps
- [ ] 5.4 Write unit test: config diff detects WASM path change
- [ ] 5.5 Write integration test: resync triggers hot-swap for changed node

## 6. Swap Validation & Safety

- [ ] 6.1 Validate new WASM component before starting drain (fail fast)
- [ ] 6.2 Add swap-in-progress lock per node (prevent concurrent swaps)
- [ ] 6.3 Implement abort path: on validation failure, log error, keep old node
- [ ] 6.4 Write test: trigger swap with invalid WASM, verify old node continues

## 7. Hot-Swap Metrics

- [ ] 7.1 Add swap timing fields to `PipelineMetrics` struct
- [ ] 7.2 Instrument prepare phase with timing capture
- [ ] 7.3 Instrument drain phase with timing capture
- [ ] 7.4 Instrument flip phase with timing capture
- [ ] 7.5 Instrument retire phase with timing capture
- [ ] 7.6 Add message count metrics (in_flight, delayed, dropped)
- [ ] 7.7 Add success/failure counters with labels
- [ ] 7.8 Write test: perform swap, verify metrics populated

## 8. Documentation & Examples

- [ ] 8.1 Update MVP.md with hot-swap status
- [ ] 8.2 Create example config showing hot-swap usage
- [ ] 8.3 Add hot-swap section to REGISTRY.md (swapping to new OCI version)
- [ ] 8.4 Update SPEC.md milestone checkboxes

## 9. Benchmarking

- [ ] 9.1 Create benchmark harness for hot-swap timing
- [ ] 9.2 Measure swap latency under idle conditions
- [ ] 9.3 Measure swap latency under load (100 msg/s, 1000 msg/s)
- [ ] 9.4 Measure message loss under timeout conditions
- [ ] 9.5 Document results and compare against SPEC targets (< 100ms)
