# WAFER HTTP API Reference

> **Version:** 0.4.0  
> **Last Updated:** 2026-02-28

The WAFER runtime exposes an HTTP API for monitoring and controlling pipeline execution.

## Base URL

Default: `http://localhost:8080`

Configure via:
- Config file: `[api] enabled = true` and `bind = "0.0.0.0:8080"`
- CLI flag: `--api-bind 0.0.0.0:8080`
- Disable API: `--no-api` flag

## Authentication

Currently no authentication. Run behind a reverse proxy for production deployments.

## Implementation Status

| Endpoint | Status | Notes |
|----------|--------|-------|
| `GET /health` | ✅ Implemented | Always available |
| `GET /ready` | ✅ Implemented | Returns pipeline state |
| `GET /api/v1/pipeline` | ✅ Implemented | Returns PipelineStatus |
| `GET /api/v1/nodes` | ✅ Implemented | Returns all NodeInfo |
| `GET /api/v1/nodes/:id` | ✅ Implemented | Returns single NodeInfo |
| `POST /api/v1/nodes/:id/hot-swap` | ✅ Implemented | Full drain-and-flip hot-swap |
| `POST /api/v1/pipeline/reload` | ✅ Implemented | Config diff & resync |
| `POST /api/v1/pipeline/drain` | ✅ Implemented | Triggers pipeline drain |
| `POST /api/v1/pipeline/shutdown` | ✅ Implemented | Triggers graceful shutdown |
| `GET /metrics` | ✅ Implemented | Full Prometheus registry (SPEC §12.2) |

---

## Health Endpoints

### GET /health

Simple liveness check. Always returns 200 if the server is running.

**Response:**
```json
{"status": "ok"}
```

**Status Codes:**
- `200 OK` - Server is alive

---

### GET /ready

Readiness check. Returns 200 if the pipeline is ready to process messages.

**Response:**
```json
{
  "ready": true,
  "state": "running"
}
```

**Status Codes:**
- `200 OK` - Ready
- `503 Service Unavailable` - Not ready (starting, draining, or stopped)

---

## Pipeline Endpoints

### GET /api/v1/pipeline

Get current pipeline status.

**Response:**
```json
{
  "name": "my-pipeline",
  "state": "running",
  "uptime_secs": 3600,
  "messages_processed": 152847,
  "messages_failed": 3,
  "nodes_running": 5,
  "nodes_draining": 0
}
```

**Pipeline States:**
- `starting` - Pipeline is initializing
- `running` - Normal operation
- `draining` - Finishing in-flight messages, not accepting new ones
- `stopped` - Pipeline has stopped
- `error` - Pipeline encountered an error

---

### POST /api/v1/pipeline/reload

Reload configuration and hot-swap changed nodes.

**Request Body:** (optional)
```json
{
  "config_path": "/path/to/new/config.toml"
}
```

If no body provided, reloads from the original config path.

**Response:**
```json
{
  "swapped_nodes": ["transform-1", "filter-2"]
}
```

**Status Codes:**
- `200 OK` - Reload successful
- `400 Bad Request` - Invalid config
- `501 Not Implemented` - Hot-swap not yet implemented

---

### POST /api/v1/pipeline/drain

Start draining the pipeline. Stops accepting new messages and waits for in-flight messages to complete.

**Response:**
```json
{
  "status": "draining",
  "message": "Pipeline is draining"
}
```

**Status Codes:**
- `200 OK` - Drain started
- `409 Conflict` - Already draining or stopped

---

### POST /api/v1/pipeline/shutdown

Initiate graceful shutdown of the pipeline.

**Response:**
```json
{
  "status": "shutting_down",
  "message": "Shutdown initiated"
}
```

**Status Codes:**
- `200 OK` - Shutdown initiated
- `409 Conflict` - Already shutting down

---

## Node Endpoints

### GET /api/v1/nodes

List all nodes in the pipeline.

**Response:**
```json
[
  {
    "id": "source",
    "node_type": "source",
    "state": "running",
    "swappable": false,
    "messages_processed": 152847,
    "messages_failed": 0,
    "avg_process_us": 12,
    "queue_depth": null
  },
  {
    "id": "transform-1",
    "node_type": "transform",
    "state": "running",
    "swappable": true,
    "messages_processed": 152844,
    "messages_failed": 2,
    "avg_process_us": 150,
    "queue_depth": 42
  }
]
```

**Node Types:**
- `source` - Data ingestion
- `transform` - Message transformation (WASM)
- `router` - 1→N content-based routing
- `joiner` - N→1 merge
- `sink` - Data output

**Node States:**
- `running` - Normal operation
- `draining` - Finishing in-flight messages
- `retired` - No longer processing
- `error` - Node encountered an error

---

### GET /api/v1/nodes/:id

Get details for a specific node.

**Response:**
```json
{
  "id": "transform-1",
  "node_type": "transform",
  "state": "running",
  "swappable": true,
  "messages_processed": 152844,
  "messages_failed": 2,
  "avg_process_us": 150,
  "queue_depth": 42
}
```

**Status Codes:**
- `200 OK` - Node found
- `404 Not Found` - Node does not exist

---

### POST /api/v1/nodes/:id/hot-swap

Trigger hot-swap on a specific node. Drains pending messages, loads new WASM module, and resumes processing.

**Response:**
```json
{
  "node_id": "transform-1",
  "drain_duration": {"secs": 1, "nanos": 234567890},
  "load_duration": {"secs": 0, "nanos": 456789012},
  "total_duration": {"secs": 1, "nanos": 691356902},
  "messages_drained": 42
}
```

**Status Codes:**
- `200 OK` - Hot-swap completed
- `404 Not Found` - Node does not exist
- `409 Conflict` - Swap already in progress
- `422 Unprocessable Entity` - Node is not swappable
- `501 Not Implemented` - Hot-swap not yet implemented

---

## Metrics Endpoint

### GET /metrics

Prometheus-compatible metrics in text format.

**Response:**
```
# HELP wafer_messages_processed_total Total messages processed
# TYPE wafer_messages_processed_total counter
wafer_messages_processed_total{node="source"} 152847
wafer_messages_processed_total{node="transform-1"} 152844
wafer_messages_processed_total{node="sink"} 152842

# HELP wafer_messages_failed_total Total messages failed
# TYPE wafer_messages_failed_total counter
wafer_messages_failed_total{node="source"} 0
wafer_messages_failed_total{node="transform-1"} 2
wafer_messages_failed_total{node="sink"} 0

# HELP wafer_queue_depth Current queue depth
# TYPE wafer_queue_depth gauge
wafer_queue_depth{queue="source_to_transform-1"} 42
wafer_queue_depth{queue="transform-1_to_sink"} 0

# HELP wafer_process_duration_us Average processing time in microseconds
# TYPE wafer_process_duration_us gauge
wafer_process_duration_us{node="transform-1"} 150
```

**Content-Type:** `text/plain; version=0.0.4`

---

## Error Responses

All error responses follow this format:

```json
{
  "error": {
    "code": "node_not_found",
    "message": "Node 'foo' does not exist",
    "details": {
      "node_id": "foo"
    }
  }
}
```

**Error Codes:**
- `node_not_found` - Referenced node does not exist
- `swap_in_progress` - A hot-swap is already running
- `not_swappable` - Node does not support hot-swap
- `not_implemented` - Feature not yet implemented
- `config_error` - Configuration is invalid
- `invalid_state` - Operation not valid in current state
- `internal` - Unexpected internal error

---

## Using with curl

```bash
# Health check
curl http://localhost:8080/health

# Get pipeline status
curl http://localhost:8080/api/v1/pipeline

# List nodes
curl http://localhost:8080/api/v1/nodes

# Get specific node
curl http://localhost:8080/api/v1/nodes/transform-1

# Trigger hot-swap (drains pending messages, swaps to new WASM, resumes)
curl -X POST http://localhost:8080/api/v1/nodes/transform-1/hot-swap

# Hot-swap with explicit WASM path
curl -X POST http://localhost:8080/api/v1/nodes/transform-1/hot-swap \
  -H "Content-Type: application/json" \
  -d '{"wasm_path": "plugins/uppercase/target/wasm32-wasip2/release/uppercase_transform.wasm"}'

# Reload config (detects changes and hot-swaps affected nodes)
curl -X POST http://localhost:8080/api/v1/pipeline/reload

# Drain pipeline
curl -X POST http://localhost:8080/api/v1/pipeline/drain

# Shutdown
curl -X POST http://localhost:8080/api/v1/pipeline/shutdown

# Get metrics
curl http://localhost:8080/metrics
```

---

## Using with waferctl

The `waferctl` CLI provides a more user-friendly interface:

```bash
# Set up endpoint (default is localhost:8080)
waferctl config set-endpoint local http://localhost:8080
waferctl config use local

# Check health
waferctl health

# Get status
waferctl status

# List nodes (table format)
waferctl nodes

# JSON output
waferctl --json status

# Trigger hot-swap
waferctl hot-swap transform-1

# Hot-swap with explicit WASM path
waferctl hot-swap transform-1 --path plugins/uppercase/target/wasm32-wasip2/release/uppercase_transform.wasm

# Get metrics (human-readable)
waferctl metrics

# Get raw Prometheus format
waferctl metrics --raw

# See waferctl --help for all commands
```

See [crates/waferctl/README.md](../crates/waferctl/README.md) for complete CLI documentation.

---

## Environment Variables

### RUST_LOG

Controls log verbosity using the `tracing` crate's filter syntax.

**Format:** `RUST_LOG=<target>=<level>,...`

**Levels:** `trace`, `debug`, `info`, `warn`, `error`

**Examples:**

```bash
# Default level (info)
cargo run -p wafer-runtime -- --config examples/dag-passthrough.toml

# Debug logging for all wafer components
RUST_LOG=debug cargo run -p wafer-runtime -- --config examples/dag-passthrough.toml

# Specific component logging
RUST_LOG=wafer_core::dag=debug,wafer_core::metrics=trace cargo run -p wafer-runtime -- --config examples/dag-passthrough.toml

# Quiet mode (warnings and errors only)
RUST_LOG=warn cargo run -p wafer-runtime -- --config examples/dag-passthrough.toml
```

**Common targets:**
- `wafer_core` - Core runtime (all modules)
- `wafer_core::dag` - DAG orchestration and node execution
- `wafer_core::metrics` - Metrics registry and collection
- `wafer_core::api` - HTTP API server
- `wafer_runtime` - Main runtime binary

### Log Format

The runtime supports two log formats via the `--log-format` CLI flag:

```bash
# Text format (default) - human-readable
cargo run -p wafer-runtime -- --config config.toml --log-format text

# JSON format - structured for log aggregators (SPEC §12.4)
cargo run -p wafer-runtime -- --config config.toml --log-format json
```

**JSON log structure:**
```json
{
  "timestamp": "2026-02-28T10:30:00.123456789Z",
  "level": "INFO",
  "target": "wafer_core::dag::runner",
  "span": {
    "pipeline": "sensor-telemetry",
    "node_id": "json-parser"
  },
  "message": "Message processed",
  "fields": {
    "message_id": "msg-12345",
    "duration_ns": 1234,
    "input_size_bytes": 256,
    "output_size_bytes": 312
  }
}
```

---

## Prometheus Integration

### Scrape Configuration

See [examples/prometheus.yml](../examples/prometheus.yml) for a ready-to-use Prometheus scrape configuration.

**Quick start:**
```bash
# Start WAFER with metrics endpoint
cargo run -p wafer-runtime -- --config examples/dag-passthrough-with-api.toml

# In another terminal, start Prometheus
prometheus --config.file=examples/prometheus.yml

# Or with Docker
docker run -d --name prometheus \
  -p 9092:9090 \
  -v $(pwd)/examples/prometheus.yml:/etc/prometheus/prometheus.yml \
  --network host \
  prom/prometheus
```

### Available Metrics

All metrics follow SPEC §12.2. See the `/metrics` endpoint for current values.

**Pipeline metrics:**
- `wafer_pipeline_uptime_seconds` - Time since pipeline started
- `wafer_pipeline_messages_total` - Total messages processed
- `wafer_pipeline_errors_total` - Total processing errors

**Node metrics (labeled by `node_id`, `node_type`):**
- `wafer_node_invocations_total` - Total invocations per node
- `wafer_node_process_time_ns` - Processing time in nanoseconds
- `wafer_node_errors_total` - Errors per node

**Queue metrics (labeled by `from`, `to`):**
- `wafer_queue_depth` - Current queue depth
- `wafer_queue_capacity` - Queue capacity
- `wafer_queue_enqueue_total` - Messages enqueued
- `wafer_queue_drop_total` - Messages dropped (overflow policy: drop)
- `wafer_queue_dlq_total` - Messages sent to DLQ (overflow policy: dead-letter)

**DLQ metrics:**
- `wafer_dlq_messages_total` - Total messages routed to DLQ
- `wafer_dlq_sink_error_total` - Errors writing to DLQ sink

**Sink metrics (labeled by `sink_id`):**
- `wafer_sink_batch_flush_total` - Total batch flushes
- `wafer_sink_batch_size` - Messages in last batch
- `wafer_sink_buffer_size` - Current buffer size

**System metrics:**
- `wafer_host_cpu_percent` - Process CPU usage
- `wafer_host_memory_rss_bytes` - Resident set size
- `wafer_host_threads` - Thread count

**Hot-swap metrics:**
- `wafer_hotswap_total` - Total hot-swap operations attempted
- `wafer_hotswap_success_total` - Total successful hot-swap operations
- `wafer_hotswap_failure_total` - Total failed hot-swap operations
- `wafer_hotswap_drain_timeout_total` - Hot-swaps where drain phase timed out
- `wafer_hotswap_prepare_time_ns_total` - Cumulative time in prepare phase
- `wafer_hotswap_drain_time_ns_total` - Cumulative time in drain phase
- `wafer_hotswap_flip_time_ns_total` - Cumulative time in flip phase
- `wafer_hotswap_retire_time_ns_total` - Cumulative time in retire phase
- `wafer_hotswap_messages_drained_total` - Total messages drained during swaps

---

## Local Development Stack

For local development and testing, a Docker Compose stack is available with Prometheus and Grafana pre-configured:

```bash
cd examples/observability

# Start Prometheus + Grafana
docker-compose up -d

# Grafana: http://localhost:3000 (admin/admin)
# Prometheus: http://localhost:9090
```

The stack includes:
- **Prometheus** scraping WAFER at `host.docker.internal:8080/metrics`
- **Grafana** with a pre-built "WAFER Overview" dashboard
- Dashboard panels for pipeline, node, queue, and hot-swap metrics

See `examples/observability/README.md` for full documentation.

---

## See Also

- [MVP.md](MVP.md) - Current implementation status
- [SPEC.md](SPEC.md) - Full specification
- [openspec/specs/rest-api/spec.md](../openspec/specs/rest-api/spec.md) - API specification
- [openspec/specs/waferctl/spec.md](../openspec/specs/waferctl/spec.md) - CLI specification
