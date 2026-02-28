## Why

Hot-swap is the **primary thesis deliverable** for WAFER. The runtime must support upgrading individual WASM nodes without stopping the entire pipeline—enabling zero-downtime upgrades, bug fixes in production, and configuration changes without restart. This capability differentiates WAFER from traditional pipeline runtimes that require full restarts for updates.

The design is already specified in SPEC.md §10 and ADR-0003, but implementation hasn't started yet. Now that the DAG execution engine is stable (M0 complete), hot-swap implementation is the critical path for thesis completion.

## What Changes

- **New hot-swap coordinator** in the DAG orchestrator that manages the drain-and-flip algorithm
- **Node state machine** to track node lifecycle states: `running`, `draining`, `retired`
- **Message routing control** to pause/resume routing to specific nodes during swap
- **Metrics collection** for swap timing (prepare, drain, flip, retire phases)
- **Timeout handling** for drain phase with configurable `drain_timeout_ms`

> **Note:** Hot-swap triggers (REST API, `waferctl reload`) are defined in the `runtime-control-plane` OpenSpec change.

## Capabilities

### New Capabilities
- `hot-swap-coordinator`: Core drain-and-flip algorithm implementation, node state machine, message routing control during swap
- `hot-swap-metrics`: Timing metrics for swap phases per SPEC §10.2

### Modified Capabilities
*(none - this is a new feature addition, no existing specs to modify)*

### Related Changes
- `runtime-control-plane`: Provides PipelineControl trait and REST API endpoints that trigger hot-swap (`/api/v1/pipeline/reload`, `/api/v1/nodes/:id/hot-swap`)
- `observability-prometheus`: Provides MetricsRegistry for exporting swap timing metrics

## Impact

**Code:**
- `crates/wafer-core/src/dag/orchestrator.rs` - Add hot-swap coordination logic
- `crates/wafer-core/src/dag/runner.rs` - Modify node execution loops to respect draining state
- `crates/wafer-core/src/node/mod.rs` - Add node state enum (Running, Draining, Retired)
- `crates/wafer-core/src/metrics/` - Add swap timing metrics (values; export via observability-prometheus)

**WIT:** No changes required - lifecycle interface already has `validate()`, `init()`, `close()`

**Config:** Uses existing `drain_timeout_ms` field in pipeline config

**Dependencies:**
- `runtime-control-plane`: Crate structure, PipelineControl trait that wraps hot-swap methods
- `observability-prometheus`: MetricsRegistry for Prometheus export of swap metrics

**SPEC Reference:** Section 10 (Hot-Swap Mechanism), ADR-0003
