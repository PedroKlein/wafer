## Why

The WAFER runtime collects metrics internally (via `PipelineMetrics` struct) but has no way to export them. For thesis evaluation and production use, we need a Prometheus-compatible metrics endpoint that can be scraped by monitoring systems. This enables:

1. **Thesis benchmarking**: Collect throughput, latency, and resource usage data across test runs
2. **Production monitoring**: Grafana dashboards, alerting, capacity planning
3. **Hot-swap validation**: Measure swap timing metrics per SPEC §10.2

The infrastructure for metrics collection already exists (`src/metrics/counters.rs`). This change adds the HTTP endpoint and Prometheus formatting.

**SPEC Reference:** Section 12.2-12.5 (Observability)

## What Changes

- **New HTTP server** for metrics and health endpoints
- **Prometheus text format** exporter for all metrics categories (pipeline, node, queue, system)
- **Health endpoints** (`/health`, `/ready`, `/live`) for orchestration compatibility
- **Configuration** for metrics server bind address and port
- **Structured JSON logging** via `tracing-subscriber` with JSON formatter

## Capabilities

### New Capabilities
- `metrics-endpoint`: HTTP server exposing `/metrics` in Prometheus format, with all pipeline/node/queue/system metrics
- `health-endpoints`: HTTP endpoints for health, readiness, and liveness checks
- `structured-logging`: JSON-formatted log output via tracing

### Modified Capabilities
*(none - builds on existing internal metrics, no spec changes)*

## Impact

**Code:**
- `src/metrics/` - Add Prometheus formatter, HTTP server
- `src/config/schema.rs` - Add metrics server config fields
- `src/main.rs` - Start metrics server alongside pipeline
- Cargo.toml - Add `axum` (HTTP), `prometheus-client` (formatting), `tracing-subscriber` with JSON

**Dependencies:**
- `axum` - Lightweight HTTP server (already async/Tokio-native)
- `prometheus-client` - Official Prometheus Rust client for text format
- `tracing-subscriber` with `json` feature

**Config:** New optional section:
```toml
[metrics]
enabled = true
bind = "0.0.0.0:9090"
```

**SPEC Reference:** Section 12.2-12.5
