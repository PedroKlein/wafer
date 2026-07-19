---
name: dag-orchestration
description: >
  DAG orchestration patterns for WAFER's pipeline topology. Covers graph construction
  with petgraph, topological sorting (Kahn's vs DFS post-order trade-offs), cycle
  detection, fan-out/fan-in validation, failure mode management (supervisor pattern with
  restart/escalate/ignore policies), end-to-end backpressure preservation across the
  DAG, port-based edge addressing, and JoinSet-based task supervision. Use when working
  with pipeline topology, graph validation, node ordering, failure handling, or extending
  the DAG structure. Triggers on: DAG, graph, petgraph, topology, topological sort,
  topo_order, cycle, orphan, fan-out, fan-in, DiGraph, NodeIndex, edge, port, routing,
  DagGraph, supervisor, restart policy, failure mode. Do NOT use for async execution
  patterns (use async-tokio) or WASM instance lifecycle (use wasm-specialist).
---

# DAG Orchestration Patterns

## Before Modifying the Graph

Ask yourself:
- **Is this topology or execution?** If it touches channels, async, or WASM → wrong layer.
  The graph knows nothing about runtime concerns.
- **Is this a new query or a new mutation?** Mutations require re-validation (toposort).
- **Does this need index stability?** If removing nodes at runtime, `StableGraph` preserves
  indices; `DiGraph` reuses them after removal (silent corruption if you hold stale indices).
- **Am I in a hot path?** DagGraph methods are synchronous O(V+E); keep them there.

---

## Execution Model Taxonomy (Validated Across 12 Systems)

WAFER's task-per-node model is unique among Wasm pipeline runtimes:

| # | Model | System | Trade-offs |
|---|-------|--------|------------|
| 1 | **Task-per-node** (bounded channels) | WAFER | Full parallelism + per-node isolation + explicit backpressure. Cost: channel allocation + context switches |
| 2 | Task-per-flow (sequential scheduling) | Torvyn | Zero sync overhead. Cost: head-of-line blocking, no per-stage parallelism |
| 3 | Stack-based frontier expansion | Flow-Like | Natural termination for one-shot workflows. Not streaming. |
| 4 | Event-loop dispatch | Wick (abandoned) | Perfect observability, simple consistency. Cost: throughput bottleneck at coordinator |
| 5 | Inline-chain-per-request | Fluvio | Simple, no coordination. Cost: shared fate, no persistence |
| 6 | Trigger-per-instance (serverless) | Spin | Perfect isolation, no state leakage. Cost: cold start, no streaming |
| 7 | Platform-mediated per-message | Azure Dataflow | Platform handles complexity. Cost: no standalone operation |
| 8 | Goroutine-per-operator | eKuiper | Lightweight concurrency. Cost: no per-operator isolation |

**Key insight**: Only models #1 (WAFER) and #8 (eKuiper) provide per-stage parallelism in a
continuous streaming context. WAFER adds per-node fault isolation via Wasm Stores; eKuiper
shares process memory across all operators.

---

## Core Design: Separation of Topology from Execution

From Meridian Space (data pipeline orchestration reference):
> "The data structure that captures 'what the pipeline looks like' is a directed acyclic graph.
> The acyclic constraint is operationally critical: a pipeline with a cycle is a pipeline that
> can deadlock under backpressure."

WAFER enforces this separation:
- **DagGraph**: Pure topology — synchronous, no async, no WASM knowledge, immutable after build
- **PipelineOrchestrator**: Execution — channels, WASM instances, async tasks, hot-swap

The graph answers structural questions. The orchestrator uses those answers to wire execution.

---

## Topological Sort: Kahn's vs DFS Post-Order

Both are O(V+E). **Kahn's is preferred for pipelines** because:
- Produces **level-by-level** order (sources first, then their consumers, then theirs)
- Matches how an operator engineer thinks about the pipeline
- DFS post-order produces less intuitive interleaving (deep chain finished before shallow sibling)

petgraph's `toposort()` uses DFS-based ordering. For WAFER, this is acceptable because
the topo order is used for init/shutdown sequencing, not for explaining the pipeline to users.

**Key insight**: Toposort IS your cycle detection. It returns `Err(Cycle(node))` if any
cycle exists. A separate `has_cycle()` call is redundant computation.

---

## Validation at Build Time, Not Runtime

The `build()` step is where validation happens — refuse to start rather than start and hope:

```rust
pub fn from_config(config: &DagConfig) -> Result<Self> {
    // 1. Add nodes → build index
    // 2. Add edges → validate endpoints exist
    // 3. Toposort → detect cycles (fail here, not at 3 AM)
    // 4. Validate no orphans (misconfigured nodes)
    // 5. Validate per-role requirements (sources have outputs, sinks have inputs)
}
```

### Validation Checklist

| Check | When It Fails |
|-------|---------------|
| Cycle detection | Toposort returns Err — deadlock under backpressure |
| Orphan detection | Node with 0 edges in multi-node graph — config typo |
| Unknown node in edge | Edge references non-existent node — config typo |
| Source has no outputs | Source produces messages nowhere — useless |
| Sink has no inputs | Sink never receives messages — useless |
| Empty graph | No nodes defined — nothing to run |

---

## Failure Mode Management (Thesis RQ2: Fault Containment)

RQ2 asks: "Do per-stage sandboxes contain faults without pipeline-wide failure?"
Pass criterion: <1% throughput impact on healthy stages when one node traps.
Attack scenarios S1-S6 validate memory isolation, CPU exhaustion, resource limits, and
blast radius. The supervisor pattern below is how the runtime reacts to trapped nodes.

From Meridian Space and Erlang/OTP principles:

A pipeline has three classes of failure that retry cannot handle:
1. **Panics**: node hits `unwrap()` on `None` — task torn down by runtime
2. **Cascading slowdowns**: one operator's degradation propagates through topology
3. **Resource exhaustion**: one misbehaving operator starves others

### Supervisor with JoinSet

```rust
// The supervisor watches all node tasks and applies per-node policies
while let Some(result) = join_set.join_next().await {
    match classify_exit(&result) {
        TaskExit::Clean => { /* expected — node finished normally */ }
        TaskExit::Panic(info) => {
            match node_policy(&info.node_id) {
                Policy::Restart => { join_set.spawn(restart_node(&info)); }
                Policy::Escalate => { cancel_token.cancel(); break; }
                Policy::Ignore => { /* sidecar nodes — metrics, debug loggers */ }
            }
        }
        TaskExit::Error(e) => {
            tracing::error!(node = %e.node_id, error = %e, "Node failed");
            // Same policy dispatch...
        }
    }
}
```

### Bulkheading (Resource Isolation)

One slow WASM plugin must not starve others. Isolation mechanisms:
- **Fuel metering**: per-call fuel limit prevents infinite loops from consuming all CPU
- **Epoch interruption**: time-bounded execution regardless of fuel
- **Separate channel per edge**: a slow consumer only backpressures its upstream, not all nodes
- **Per-node metrics**: detect degradation before it cascades

---

## Port-Based Edge Addressing

Edges use `"node_id:port_name"` format with default port `"default"`:

```rust
fn parse_node_port(key: &str) -> (&str, &str) {
    key.split_once(':').unwrap_or((key, "default"))
}
```

### Fan-Out (Router)
```toml
[[edges]]
from = "router"
port = "high"       # Router's WASM route() returns this port name
to = "priority-sink"

[[edges]]
from = "router"
port = "low"        # router output port (only used when 'from' is a router)
to = "batch-transform"
```

### Fan-In (implicit multi-producer topology)
Multiple upstream nodes → single downstream node. There is no first-class
Joiner node type; fan-in is expressed by wiring several upstream senders onto
the downstream node's single bounded `mpsc` receiver (`mpsc::channel` is
naturally multi-producer / single-consumer). The graph builder handles this
automatically when a node has multiple inbound edges.

---

## Queue Wiring from Graph

Queues are created per-edge at build time. The graph-level invariant:
**Every edge is a bounded channel** — no unbounded channels anywhere in the graph builder.

```rust
for edge in &config.edges {
    let capacity = edge.queue_capacity.unwrap_or(DEFAULT_QUEUE_CAPACITY);
    let (tx, rx) = BoundedQueue::new(capacity).split();
    senders.insert((from_key, to_key), tx);
    receivers.insert((from_key, to_key), rx);
}
```

**After spawning all tasks, drop remaining senders** — this ensures downstream receivers
see channel close when upstream tasks finish, propagating clean shutdown through the DAG.

---

## Petgraph Type Selection (Validated by wasm-compose + 12-Repo Review)

| Type | Use When | WAFER Status |
|------|----------|----------|
| `DiGraph<N, E>` | Fixed topology, no runtime removal | ✅ Current & correct (validated) |
| `StableGraph<N, E>` | Runtime node add/remove (index stability) | NOT needed — hot-swap replaces Wasm instance, not graph node |
| `Acyclic<G>` | Dynamic edges with cycle prevention (Pierce-Kelly algorithm) | 🔮 Future: dynamic topology |
| `GraphMap<N, E>` | Value-keyed nodes (Copy+Ord+Hash constraint) | ❌ Wrong constraints for WAFER |

**Key correction**: Hot-swap replaces the WASM instance, NOT the graph node. The topology
remains fixed. `DiGraph` is correct; `StableGraph` is unnecessary overhead.

**Optimization**: `Ix = u16` type parameter halves index memory. For WAFER's edge IoT targets
(Pi4, Jetson) with pipelines of <100 nodes, this is free optimization.

### Useful Algorithms Not Yet Used

| Algorithm | Use Case | API | Priority |
|-----------|----------|-----|----------|
| `has_path_connecting` | Hot-swap reachability safety check | `algo::has_path_connecting(g, from, to, space)` | ✅ Should adopt |
| `connected_components` | Validate graph connectivity | `algo::connected_components()` | Low |
| `dominators` | Find single-point-of-failure nodes | `algo::dominators::*` | Low |
| `Topo` walker | Incremental topo iteration | `visit::Topo` | Not needed (batch toposort sufficient) |

### XOR Hash Deadlock Detection (from flow-like)

For iterative DAG executors, XOR all node pointer keys. If hash unchanged between steps,
no progress was made → deadlock. O(1) time and space. Not needed for WAFER (single-pass
DAG, not iterative) but valuable reference for extending architecture.

---

## NEVER

- **NEVER store execution state in the graph** — DagGraph is pure topology; WASM
  instances, channels, and metrics live in the orchestrator/runner layer
- **NEVER use `NodeIndex` as a persistent identifier** — DiGraph's `swap_remove` on
  `remove_node` causes the last node to take the removed node's slot, silently corrupting
  any external data structure indexing by NodeIndex; use String IDs in the `node_indices` map
- **NEVER allow an unbounded channel anywhere in the DAG** — one unbounded link breaks
  end-to-end backpressure for the entire pipeline (Meridian Space: "the unbounded channel
  acts as an infinite buffer; stages downstream OOM while upstream thinks everything is fine")
- **NEVER skip build-time validation** — refuse to start with a broken topology rather
  than discovering the cycle/orphan/dead-edge at 3 AM in production
- **NEVER add edges without checking both endpoints exist** — petgraph silently accepts
  invalid NodeIndex values; always validate via the `node_indices` map first
- **NEVER assume topo_order is unique** — multiple valid orderings exist for the same DAG;
  WAFER's order is deterministic (insertion order breaks ties) but not canonical
- **NEVER use `Vec<JoinHandle>` with sequential await for supervision** — serializes on
  the slowest task; use `JoinSet::join_next()` to react to whichever finishes first
