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

### D1: HTTP Framework - Axum

Use `axum` for the HTTP server:

```rust
let app = Router::new()
    .route("/metrics", get(metrics_handler))
    .route("/health", get(health_handler))
    .route("/ready", get(ready_handler))
    .route("/live", get(live_handler));
```

**Rationale:** Axum is async-native (Tokio), lightweight, and well-maintained by the Tokio team. Alternatives considered:
- `warp`: Similar capability but more complex API
- `actix-web`: Different async runtime, adds complexity
- `hyper` raw: Too low-level for our needs

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

### D6: Optional Metrics Server

Metrics server is opt-in via config:

```toml
[metrics]
enabled = true
bind = "127.0.0.1:9090"
```

**Rationale:** Not all deployments need metrics exposure. Default to disabled to minimize attack surface.

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

- [ ] **Q1:** Should metrics server run on a separate port or share with future REST API?
  - Current design: separate port (9090 default)
  - Alternative: shared port with path routing (/api/*, /metrics)
