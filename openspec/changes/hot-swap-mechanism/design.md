## Context

The WAFER runtime currently executes DAG pipelines with WASM transform nodes, but lacks the ability to update nodes at runtime. When a node needs updating (bug fix, config change, new version), the entire pipeline must be stopped. This is unacceptable for production edge deployments.

**Current state:**
- DAG orchestrator spawns node execution loops as Tokio tasks
- Each node has a dedicated input queue (`BoundedQueue<RuntimeEnvelope>`)
- Messages flow through queues; backpressure propagates naturally
- No mechanism exists to pause message routing to a specific node

**Key constraint:** WASM stores are `Send` but not `Sync`, so each node instance runs on a single task. Hot-swap must coordinate with this single-task-per-node model.

## Goals / Non-Goals

**Goals:**
- Implement drain-and-flip algorithm per SPEC §10.1 and ADR-0003
- Support hot-swap for WASM Transform nodes (primary use case)
- Measure swap timing (< 100ms target under normal load)
- Handle timeout gracefully (force swap, log dropped count)
- Expose `hot_swap()` method on orchestrator for triggers to call

**Non-Goals:**
- Stateful hot-swap (state migration between v1 and v2) - future work
- Hot-swap for native Source/Sink nodes - native code, use process restart
- Automatic rollback if v2 fails after swap - future work
- Hot-swap for Router/Joiner nodes - same mechanism, test separately
- Trigger mechanisms (REST API, CLI) - see `control-plane` change

## Decisions

### D1: Node State Machine

Add explicit lifecycle states to nodes:

```rust
enum NodeState {
    Starting,     // Being initialized
    Running,      // Normal operation
    Draining,     // No new messages, processing in-flight
    Retired,      // Closed, awaiting cleanup
}
```

**Rationale:** Explicit states make the drain-and-flip phases observable and debuggable. Alternative (implicit via booleans) was rejected for clarity.

### D2: Drain Detection via Queue Depth

Detect "drain complete" by checking:
1. Input queue is empty
2. Node is not currently processing a message

**Implementation:** The node execution loop sets a `processing: AtomicBool` flag around each `process()` call. Drain is complete when `queue.is_empty() && !processing.load()`.

**Alternative considered:** Counting in-flight messages. Rejected because it adds complexity and races - checking queue + processing flag is simpler.

### D3: Message Routing Control

Add a `routing_enabled: AtomicBool` per node that the sender checks before enqueuing:

```rust
// In sender (upstream node's output logic)
if downstream.routing_enabled.load(Ordering::Acquire) {
    downstream.input_queue.send(envelope).await;
} else {
    // Buffer for new node or drop if swap aborted
}
```

**Rationale:** AtomicBool is lock-free and fast. Alternative (channel close/reopen) was considered but complicates the happy path.

### D4: Hot-Swap Coordinator Location

Place hot-swap coordination in `DagOrchestrator`, not a separate module:

```rust
impl DagOrchestrator {
    pub async fn hot_swap(&mut self, node_id: NodeId, new_wasm: &Path) -> Result<SwapMetrics> {
        // 1. PREPARE: Load and init new component
        // 2. DRAIN: Set routing_enabled=false, wait for drain
        // 3. FLIP: Swap node reference atomically  
        // 4. RETIRE: Close old node
    }
}
```

**Rationale:** Orchestrator already owns node references and queue topology. Adding swap logic here keeps related code together. Alternative (separate SwapManager) adds indirection without benefit.

### D5: Config Diff Detection

Compare old and new config to detect what changed:

```rust
struct ConfigDiff {
    nodes_to_swap: Vec<(NodeId, PathBuf)>,  // (node, new_wasm_path)
    // Future: nodes_to_add, nodes_to_remove, edges_changed
}
```

For MVP, only detect WASM path changes for existing nodes. Topology changes are out of scope.

> **Note:** The trigger mechanisms (REST API via `waferctl reload`, `hot-swap`) are defined in the `runtime-control-plane` change. This change focuses on the core `hot_swap()` algorithm that `PipelineControl` delegates to.

## Risks / Trade-offs

| Risk | Impact | Mitigation |
|------|--------|------------|
| Drain timeout under high load | Messages dropped | Log count, make timeout configurable, recommend lowering load before swap |
| v2 fails init | Swap aborted, user confused | Clear error message, keep v1 running unchanged |
| Concurrent swap requests | Race conditions | Swap-in-progress lock per node |
| Memory spike (v1 + v2 both loaded) | OOM on constrained devices | Documented limit, swap one node at a time |

**Trade-off: Simplicity vs Features**
- Chose stateless-only swap over stateful (simpler, covers 90% of use cases)
- Chose explicit trigger (REST API) over automatic (file watch) for user control
- Chose single-node swap over batch (simpler coordination)

## Open Questions

- [ ] **Q1:** Should swap queue buffered messages during drain, or make senders wait? 
  - Current design: senders wait (backpressure). May cause upstream backup.
  - Alternative: buffer in coordinator. Adds memory pressure.
  
- [ ] **Q2:** What's the right default for `drain_timeout_ms`? 
  - SPEC says 5000ms. Need to validate with benchmarks.
