# Observability Implementation Tasks

## 1. Dependencies & Setup

- [ ] 1.1 Add `axum` dependency to Cargo.toml (HTTP server)
- [ ] 1.2 Add `prometheus-client` dependency (metrics formatting)
- [ ] 1.3 Add `tracing-subscriber` with `json` feature
- [ ] 1.4 Add `sysinfo` crate for system metrics (CPU, memory)

## 2. Metrics Registry

- [ ] 2.1 Create `src/metrics/registry.rs` with `MetricsRegistry` struct
- [ ] 2.2 Define Prometheus metric types for pipeline metrics (counters, gauges)
- [ ] 2.3 Define Prometheus metric types for node metrics (with labels)
- [ ] 2.4 Define Prometheus metric types for queue metrics (with labels)
- [ ] 2.5 Wire existing `PipelineMetrics` to feed Prometheus registry
- [ ] 2.6 Write unit tests for metrics registration and encoding

## 3. System Metrics Collection

- [ ] 3.1 Add system metrics collector using `sysinfo` crate
- [ ] 3.2 Implement `host_cpu_percent` gauge
- [ ] 3.3 Implement `host_memory_rss_bytes` gauge
- [ ] 3.4 Implement `host_threads` gauge
- [ ] 3.5 Add periodic refresh for system metrics (every scrape)

## 4. HTTP Server

- [ ] 4.1 Create `src/metrics/server.rs` with Axum router setup
- [ ] 4.2 Implement `/metrics` endpoint handler (Prometheus format)
- [ ] 4.3 Implement `/health` endpoint handler
- [ ] 4.4 Implement `/ready` endpoint handler
- [ ] 4.5 Implement `/live` endpoint handler
- [ ] 4.6 Add graceful shutdown handling
- [ ] 4.7 Write integration tests for each endpoint

## 5. Configuration

- [ ] 5.1 Add `MetricsConfig` struct to `src/config/schema.rs`
- [ ] 5.2 Add `metrics` section parsing to config loader
- [ ] 5.3 Make metrics server optional (disabled when section absent)
- [ ] 5.4 Add config validation for bind address format
- [ ] 5.5 Update example configs with metrics section

## 6. Structured Logging

- [ ] 6.1 Configure `tracing-subscriber` with JSON formatter in main.rs
- [ ] 6.2 Add span fields for pipeline and node context
- [ ] 6.3 Add structured fields to message processing logs
- [ ] 6.4 Verify log output matches SPEC §12.4 structure
- [ ] 6.5 Write test validating JSON log format

## 7. Integration

- [ ] 7.1 Start metrics server from main.rs when enabled
- [ ] 7.2 Pass `MetricsRegistry` to DagOrchestrator
- [ ] 7.3 Wire node execution to update metrics
- [ ] 7.4 Wire queue operations to update metrics
- [ ] 7.5 Add hot-swap metrics (from hot-swap-mechanism change)

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
