# Observability Implementation Tasks

> **Dependencies:** HTTP server and `/metrics` endpoint are provided by `runtime-control-plane`.
> This change implements the MetricsRegistry and Prometheus formatting that the endpoint calls.

## 1. Dependencies & Setup

- [ ] 1.1 Add `prometheus-client` dependency to `crates/wafer-core/Cargo.toml`
- [ ] 1.2 Add `tracing-subscriber` with `json` feature
- [ ] 1.3 Add `sysinfo` crate for system metrics (CPU, memory)

## 2. Metrics Registry

- [ ] 2.1 Create `crates/wafer-core/src/metrics/registry.rs` with `MetricsRegistry` struct
- [ ] 2.2 Define Prometheus metric types for pipeline metrics (counters, gauges)
- [ ] 2.3 Define Prometheus metric types for node metrics (with labels)
- [ ] 2.4 Define Prometheus metric types for queue metrics (with labels)
- [ ] 2.5 Wire existing `PipelineMetrics` to feed Prometheus registry
- [ ] 2.6 Add `encode() -> String` method returning Prometheus text format
- [ ] 2.7 Write unit tests for metrics registration and encoding

## 3. System Metrics Collection

- [ ] 3.1 Add system metrics collector using `sysinfo` crate
- [ ] 3.2 Implement `host_cpu_percent` gauge
- [ ] 3.3 Implement `host_memory_rss_bytes` gauge
- [ ] 3.4 Implement `host_threads` gauge
- [ ] 3.5 Add periodic refresh for system metrics (every scrape)

## 4. HTTP Server Integration

> **Note:** HTTP server and endpoints are implemented in `runtime-control-plane`.
> Tasks here ensure the registry integrates correctly with that server.

- [ ] 4.1 Export `MetricsRegistry` from `wafer-core` public API
- [ ] 4.2 Verify registry `encode()` output matches Prometheus text format spec
- [ ] 4.3 Write integration test: create registry, encode, parse with prom-parse
- [ ] 4.4 Document thread-safety requirements for registry access

## 5. Configuration

> **Note:** HTTP server config (bind address, port) is in `runtime-control-plane`.
> This section covers metrics-specific config (labels, collection intervals).

- [ ] 5.1 Add `MetricsConfig` struct to `crates/wafer-core/src/config/schema.rs`
- [ ] 5.2 Add global labels config (e.g., `environment`, `cluster`)
- [ ] 5.3 Add system metrics collection interval config
- [ ] 5.4 Update example configs with metrics section

## 6. Structured Logging

- [ ] 6.1 Configure `tracing-subscriber` with JSON formatter in main.rs
- [ ] 6.2 Add span fields for pipeline and node context
- [ ] 6.3 Add structured fields to message processing logs
- [ ] 6.4 Verify log output matches SPEC §12.4 structure
- [ ] 6.5 Write test validating JSON log format

## 7. Integration

- [ ] 7.1 Pass `MetricsRegistry` to `PipelineControl` (from runtime-control-plane)
- [ ] 7.2 Wire node execution to update metrics
- [ ] 7.3 Wire queue operations to update metrics
- [ ] 7.4 Add hot-swap metrics (from hot-swap-mechanism change)
- [ ] 7.5 Verify metrics available via `/metrics` endpoint (integration test with runtime-control-plane)

## 8. Documentation & Examples

- [ ] 8.1 Update MVP.md with observability status
- [ ] 8.2 Create example Prometheus scrape config
- [ ] 8.3 Document environment variables (RUST_LOG)
- [ ] 8.4 Update SPEC.md milestone checkboxes

## 9. Validation

- [ ] 9.1 Test metrics scrape with Prometheus server
- [ ] 9.2 Verify all SPEC §12.2 metrics are present
- [ ] 9.3 Load test metrics endpoint (no performance regression)
- [ ] 9.4 Verify structured logs parse correctly in jq
