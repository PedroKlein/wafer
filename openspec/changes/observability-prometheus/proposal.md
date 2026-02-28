## Why

The WAFER runtime collects metrics internally (via `PipelineMetrics` struct) but has no way to export them. For thesis evaluation and production use, we need a Prometheus-compatible metrics endpoint that can be scraped by monitoring systems. This enables:

1. **Thesis benchmarking**: Collect throughput, latency, and resource usage data across test runs
2. **Production monitoring**: Grafana dashboards, alerting, capacity planning
3. **Hot-swap validation**: Measure swap timing metrics per SPEC §10.2

The infrastructure for metrics collection already exists (`src/metrics/counters.rs`). This change adds the **MetricsRegistry**, Prometheus formatting, and system metrics collection. The HTTP transport (`/metrics` endpoint) is provided by the `runtime-control-plane` change.

**SPEC Reference:** Section 12.2-12.5 (Observability)

**Related Changes:**
- `runtime-control-plane`: Provides HTTP server and `/metrics` endpoint transport
- `hot-swap-mechanism`: Produces swap timing metrics consumed by this registry

## What Changes

- **MetricsRegistry** struct that bridges internal counters to Prometheus format
- **Prometheus text format** exporter for all metrics categories (pipeline, node, queue, system)
- **System metrics collection** via `sysinfo` crate (CPU, memory, threads)
- **Structured JSON logging** via `tracing-subscriber` with JSON formatter

> **Note:** HTTP server, `/metrics` endpoint, and health endpoints are provided by the 
> `runtime-control-plane` change. This change provides the registry and formatting that
> the endpoint handler calls.

## Capabilities

### New Capabilities
- `metrics-registry`: Central registry that collects pipeline/node/queue/system metrics and formats them for Prometheus
- `system-metrics`: Host-level metrics (CPU, memory, threads) via sysinfo crate
- `structured-logging`: JSON-formatted log output via tracing

### Modified Capabilities
*(none - builds on existing internal metrics, no spec changes)*

### Capabilities from Other Changes
- `metrics-endpoint`: HTTP `/metrics` endpoint (from `runtime-control-plane`)
- `health-endpoints`: HTTP health/ready/live endpoints (from `runtime-control-plane`)

## Impact

**Code:**
- `crates/wafer-core/src/metrics/registry.rs` - MetricsRegistry struct
- `crates/wafer-core/src/metrics/prometheus.rs` - Prometheus text format encoding
- `crates/wafer-core/src/metrics/system.rs` - System metrics via sysinfo
- `crates/wafer-core/Cargo.toml` - Add `prometheus-client`, `sysinfo`, `tracing-subscriber` with JSON

**Dependencies:**
- `prometheus-client` - Official Prometheus Rust client for text format
- `sysinfo` - System metrics collection (CPU, memory, threads)
- `tracing-subscriber` with `json` feature

**Integration with runtime-control-plane:**
The HTTP server and `/metrics` endpoint are defined in `runtime-control-plane`. This change provides:
- `MetricsRegistry::encode()` method that returns Prometheus text format
- Registry is passed to HTTP handlers via shared state

**SPEC Reference:** Section 12.2-12.5
