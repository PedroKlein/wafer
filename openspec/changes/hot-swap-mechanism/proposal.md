## Why

Hot-swap is the **primary thesis deliverable** for WAFER. The runtime must support upgrading individual WASM nodes without stopping the entire pipeline—enabling zero-downtime upgrades, bug fixes in production, and configuration changes without restart. This capability differentiates WAFER from traditional pipeline runtimes that require full restarts for updates.

The design is already specified in SPEC.md §10 and ADR-0003, but implementation hasn't started yet. Now that the DAG execution engine is stable (M0 complete), hot-swap implementation is the critical path for thesis completion.

## What Changes

- **New hot-swap coordinator** in the DAG orchestrator that manages the drain-and-flip algorithm
- **Node state machine** to track node lifecycle states: `running`, `draining`, `retired`
- **Message routing control** to pause/resume routing to specific nodes during swap
- **Swap trigger API** - initially via config file change, later REST API
- **Metrics collection** for swap timing (prepare, drain, flip, retire phases)
- **Timeout handling** for drain phase with configurable `drain_timeout_ms`

## Capabilities

### New Capabilities
- `hot-swap-coordinator`: Core drain-and-flip algorithm implementation, node state machine, message routing control during swap
- `hot-swap-triggers`: Mechanisms to trigger hot-swap (config file watch, future REST API)
- `hot-swap-metrics`: Timing metrics for swap phases per SPEC §10.2

### Modified Capabilities
*(none - this is a new feature addition, no existing specs to modify)*

## Impact

**Code:**
- `src/dag/orchestrator.rs` - Add hot-swap coordination logic
- `src/dag/runner.rs` - Modify node execution loops to respect draining state
- `src/node/mod.rs` - Add node state enum (Running, Draining, Retired)
- `src/config/` - Add config file watch for swap triggers
- `src/metrics/` - Add swap timing metrics

**WIT:** No changes required - lifecycle interface already has `validate()`, `init()`, `close()`

**Config:** Uses existing `drain_timeout_ms` field in pipeline config

**Dependencies:** May need `notify` crate for file watching

**SPEC Reference:** Section 10 (Hot-Swap Mechanism), ADR-0003
