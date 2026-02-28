# WAFER HTTP API Reference

The WAFER runtime exposes an HTTP API for monitoring and controlling pipeline execution.

## Base URL

Default: `http://localhost:9090`

Configure via:
- Config file: `[api] bind = "127.0.0.1:9090"`
- CLI flag: `--api-bind 0.0.0.0:9090`

## Authentication

Currently no authentication. Run behind a reverse proxy for production deployments.

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
curl http://localhost:9090/health

# Get pipeline status
curl http://localhost:9090/api/v1/pipeline

# List nodes
curl http://localhost:9090/api/v1/nodes

# Get specific node
curl http://localhost:9090/api/v1/nodes/transform-1

# Trigger hot-swap
curl -X POST http://localhost:9090/api/v1/nodes/transform-1/hot-swap

# Reload config
curl -X POST http://localhost:9090/api/v1/pipeline/reload

# Drain pipeline
curl -X POST http://localhost:9090/api/v1/pipeline/drain

# Shutdown
curl -X POST http://localhost:9090/api/v1/pipeline/shutdown

# Get metrics
curl http://localhost:9090/metrics
```

---

## Using with waferctl

The `waferctl` CLI provides a more user-friendly interface:

```bash
# Set up endpoint
waferctl config set-endpoint local http://localhost:9090
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

# See waferctl --help for all commands
```

See [crates/waferctl/README.md](../crates/waferctl/README.md) for complete CLI documentation.
