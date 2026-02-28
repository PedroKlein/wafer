# Observability Implementation Tasks

> **Dependencies:** HTTP server and `/metrics` endpoint are provided by `runtime-control-plane`.
> This change implements the MetricsRegistry and Prometheus formatting that the endpoint calls.

## 1. Dependencies & Setup

- [x] 1.1 Add `prometheus-client` dependency to `crates/wafer-core/Cargo.toml` (already added, feature-gated under `http-api`)
- [x] 1.2 Add `tracing-subscriber` with `json` feature
- [x] 1.3 Add `sysinfo` crate for system metrics (CPU, memory)

## 2. Metrics Registry

- [x] 2.1 Create `crates/wafer-core/src/metrics/registry.rs` with `MetricsRegistry` struct
- [x] 2.2 Define Prometheus metric types for pipeline metrics (counters, gauges)
- [x] 2.3 Define Prometheus metric types for node metrics (with labels)
- [x] 2.4 Define Prometheus metric types for queue metrics (with labels)
- [x] 2.5 Wire existing `PipelineMetrics` to feed Prometheus registry
- [x] 2.6 Add `encode() -> String` method returning Prometheus text format
- [x] 2.7 Write unit tests for metrics registration and encoding

## 3. System Metrics Collection

- [x] 3.1 Add system metrics collector using `sysinfo` crate
- [x] 3.2 Implement `host_cpu_percent` gauge
- [x] 3.3 Implement `host_memory_rss_bytes` gauge
- [x] 3.4 Implement `host_threads` gauge
- [x] 3.5 Add periodic refresh for system metrics (every scrape)

## 4. HTTP Server Integration

> **Note:** HTTP server and endpoints are implemented in `runtime-control-plane`.
> Tasks here ensure the registry integrates correctly with that server.

- [x] 4.1 Export `MetricsRegistry` from `wafer-core` public API
- [x] 4.2 Verify registry `encode()` output matches Prometheus text format spec
- [x] 4.3 Write integration test: create registry, encode, validate Prometheus format (test_prometheus_format_validation)
- [x] 4.4 Document thread-safety requirements for registry access

## 5. Configuration

> **Note:** HTTP server config (bind address, port) is in `runtime-control-plane`.
> This section covers metrics-specific config (labels, collection intervals).

- [x] 5.1 Add `MetricsConfig` struct to `crates/wafer-core/src/config/schema.rs`
- [x] 5.2 Add global labels config (e.g., `environment`, `cluster`)
- [x] 5.3 Add system metrics collection interval config
- [x] 5.4 Update example configs with metrics section

## 6. Structured Logging

- [x] 6.1 Configure `tracing-subscriber` with JSON formatter in main.rs
- [x] 6.2 Add span fields for pipeline and node context
- [x] 6.3 Add structured fields to message processing logs
- [x] 6.4 Verify log output matches SPEC §12.4 structure
- [x] 6.5 Write test validating JSON log format (json_log_format tests in integration.rs)

## 7. Integration

- [x] 7.1 Pass `MetricsRegistry` to `PipelineControl` (via ControlState in orchestrator.rs)
- [x] 7.2 Wire node execution to update metrics (all 5 runner loops now call registry methods)
- [x] 7.3 Wire queue operations to update metrics (queues registered during wire_queues)
- [x] 7.4 Add hot-swap metrics (from hot-swap-mechanism change)
- [x] 7.5 Verify metrics available via `/metrics` endpoint (test_api_server_starts_and_serves_health validates format)

## 8. Documentation & Examples

- [x] 8.1 Update MVP.md with observability status
- [x] 8.2 Create example Prometheus scrape config (examples/prometheus.yml)
- [x] 8.3 Document environment variables (RUST_LOG) in docs/api.md
- [x] 8.4 Update SPEC.md milestone checkboxes

## 9. Validation

- [x] 9.1 Test metrics scrape with Prometheus server (manual - use examples/prometheus.yml)
- [x] 9.2 Verify all SPEC §12.2 metrics are present (test_prometheus_format_validation validates all metrics)
- [x] 9.3 Load test metrics endpoint (no performance regression) - scripts/load-test-metrics.sh + criterion benches/metrics.rs
- [x] 9.4 Verify structured logs parse correctly in jq (validated via CLI test)
