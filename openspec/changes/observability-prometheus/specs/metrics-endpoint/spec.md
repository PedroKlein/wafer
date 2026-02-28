# Metrics Endpoint

HTTP endpoint exposing runtime metrics in Prometheus text format.

**SPEC Reference:** Section 12.2-12.3

## ADDED Requirements

### Requirement: Prometheus metrics endpoint

The runtime SHALL expose a `/metrics` HTTP endpoint that returns metrics in Prometheus text exposition format.

#### Scenario: Successful metrics scrape
- **WHEN** an HTTP GET request is made to `/metrics`
- **THEN** the response status is 200 OK
- **THEN** the response Content-Type is `text/plain; version=0.0.4; charset=utf-8`
- **THEN** the response body contains metrics in Prometheus text format

#### Scenario: Metrics endpoint disabled
- **WHEN** metrics server is disabled in configuration (`metrics.enabled = false`)
- **THEN** no HTTP server is started
- **THEN** no port is bound for metrics

### Requirement: Pipeline-level metrics

The metrics endpoint SHALL expose pipeline-level metrics as defined in SPEC §12.2.

#### Scenario: Pipeline uptime metric
- **WHEN** `/metrics` is scraped
- **THEN** `pipeline_uptime_seconds` gauge is present
- **THEN** value reflects seconds since pipeline started

#### Scenario: Pipeline throughput metrics
- **WHEN** `/metrics` is scraped after processing messages
- **THEN** `pipeline_messages_total` counter is present with total count
- **THEN** `pipeline_messages_per_second` gauge shows current rate
- **THEN** `pipeline_errors_total` counter shows error count

### Requirement: Node-level metrics

The metrics endpoint SHALL expose per-node metrics with `node_id` and `node_type` labels.

#### Scenario: Node invocation metrics
- **WHEN** `/metrics` is scraped after nodes process messages
- **THEN** `node_invocations_total{node_id="...",node_type="..."}` counter is present
- **THEN** `node_errors_total{node_id="...",error_code="..."}` counter is present

#### Scenario: Node timing histogram
- **WHEN** `/metrics` is scraped
- **THEN** `node_process_time_ns` histogram is present with buckets
- **THEN** histogram includes `_bucket`, `_sum`, and `_count` suffixes

#### Scenario: Node resource metrics
- **WHEN** `/metrics` is scraped
- **THEN** `node_fuel_consumed{node_id="..."}` counter shows Wasmtime fuel used
- **THEN** `node_memory_bytes{node_id="..."}` gauge shows current memory

### Requirement: Queue-level metrics

The metrics endpoint SHALL expose per-queue metrics with `from` and `to` labels.

#### Scenario: Queue depth metrics
- **WHEN** `/metrics` is scraped
- **THEN** `queue_depth{from="...",to="..."}` gauge shows current depth
- **THEN** `queue_capacity{from="...",to="..."}` gauge shows max capacity

#### Scenario: Queue throughput metrics
- **WHEN** `/metrics` is scraped after message flow
- **THEN** `queue_enqueue_total{from="...",to="..."}` counter shows enqueue count
- **THEN** `queue_drop_total{from="...",to="..."}` counter shows dropped count

### Requirement: System-level metrics

The metrics endpoint SHALL expose host system metrics.

#### Scenario: System resource metrics
- **WHEN** `/metrics` is scraped
- **THEN** `host_cpu_percent` gauge shows process CPU usage
- **THEN** `host_memory_rss_bytes` gauge shows resident set size
- **THEN** `host_threads` gauge shows thread count

### Requirement: Metrics server configuration

The metrics server SHALL be configurable via pipeline configuration.

#### Scenario: Custom bind address
- **WHEN** config specifies `metrics.bind = "0.0.0.0:8080"`
- **THEN** metrics server binds to port 8080 on all interfaces

#### Scenario: Default configuration
- **WHEN** metrics section is absent from config
- **THEN** metrics server is disabled (not started)
