## Context

WAFER already has internal metrics collection via `PipelineMetrics` struct with atomic counters for message counts and timing. However:

1. No way to observe metrics from outside the process
2. Benchmarking requires manual log parsing
3. No integration with standard monitoring tools (Prometheus/Grafana)

SPEC §12 defines the metrics schema and endpoints. This design implements that specification.

**Current state:**
- `src/metrics/counters.rs` has `PipelineMetrics` with atomic counters
- Metrics are collected but not exposed
- No HTTP server in the runtime

## Goals / Non-Goals

**Goals:**
- Expose `/metrics` endpoint in Prometheus text format
- Include all metrics defined in SPEC §12.2 (pipeline, node, queue, system)
- Add health/ready/live endpoints for orchestration
- Enable structured JSON logging
- Make metrics server optional (disabled by default)

**Non-Goals:**
- Push-based metrics (Prometheus is pull-based)
- OpenTelemetry/OTLP export (future work per SPEC)
- Distributed tracing (single-process focus)
- Custom Grafana dashboards (out of scope, user responsibility)

## Decisions

### D1: HTTP Server - Delegated to runtime-control-plane

> **Note:** The HTTP server and endpoints are now owned by the `runtime-control-plane` change.
> This change provides the `MetricsRegistry` and Prometheus encoding that the endpoint calls.

The `/metrics` endpoint handler (in runtime-control-plane) calls:

```rust
async fn metrics_handler(State(registry): State<Arc<MetricsRegistry>>) -> String {
    registry.encode() // Returns Prometheus text format
}
```

See `openspec/changes/runtime-control-plane/specs/control-api/spec.md` for HTTP server details.

### D2: Prometheus Client Library

Use `prometheus-client` (official Rust client):

```rust
use prometheus_client::metrics::counter::Counter;
use prometheus_client::registry::Registry;
use prometheus_client::encoding::text::encode;
```

**Rationale:** Official library ensures format compatibility. Alternative (`prometheus` crate) is older and less maintained.

### D3: Metrics Registry Architecture

Create a global `MetricsRegistry` that wraps both:
- Existing `PipelineMetrics` (atomic counters)
- New `prometheus-client` registry (for export)

```rust
pub struct MetricsRegistry {
    /// Internal counters (existing)
    pub pipeline: PipelineMetrics,
    /// Prometheus registry for export
    prometheus: prometheus_client::registry::Registry,
}
```

**Rationale:** Avoid duplicating counters. Pipeline code updates `PipelineMetrics`; the Prometheus registry reads from it on scrape.

### D4: Lazy Metrics Collection

Populate Prometheus metrics on-demand at scrape time rather than continuously:

```rust
async fn metrics_handler(State(registry): State<Arc<MetricsRegistry>>) -> String {
    // Read current values from PipelineMetrics
    // Format as Prometheus text
    encode_to_string(&registry.prometheus)
}
```

**Rationale:** Avoids overhead of maintaining two counter sets. Scrape frequency (typically 15s) is slow enough that reading atomics is negligible.

### D5: Structured Logging

Configure `tracing-subscriber` with JSON output:

```rust
tracing_subscriber::fmt()
    .json()
    .with_span_events(FmtSpan::CLOSE)
    .init();
```

**Rationale:** Matches SPEC §12.4 log structure. JSON is easily parseable by log aggregation systems.

### D6: Metrics Configuration - Delegated to runtime-control-plane

> **Note:** Metrics server configuration is now in `runtime-control-plane` as part of the 
> HTTP server config. Metrics are always collected internally; the HTTP exposure is optional.

See `openspec/changes/runtime-control-plane/design.md` D4 for metrics port configuration.

## Risks / Trade-offs

| Risk | Impact | Mitigation |
|------|--------|------------|
| HTTP server adds attack surface | Security | Disabled by default, bind to localhost |
| Metrics scrape under high load | Performance | Lazy collection, atomic reads are fast |
| Axum adds binary size | Deployment | ~200KB increase, acceptable for the value |
| Metric cardinality explosion | Memory | Limit node_id labels, document best practices |

**Trade-off: Pull vs Push**
- Chose pull (Prometheus scrape) over push (StatsD, OTLP)
- Simpler, no additional infrastructure needed
- Push can be added later via OTLP exporter

## Open Questions

- [x] **Q1:** Should metrics server run on a separate port or share with future REST API?
  - **Resolved:** See `runtime-control-plane` D4 - optionally separate metrics port, default shares with API
