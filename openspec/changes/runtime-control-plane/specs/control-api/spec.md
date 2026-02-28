# Control API Specification

HTTP REST API exposing PipelineControl operations. This is a feature-flagged capability (`http-api` feature in wafer-core).

## ADDED Requirements

### Requirement: Health endpoint

The system SHALL expose `GET /health` returning 200 OK when the HTTP server is running. This is a liveness check.

#### Scenario: Health check passes

- **WHEN** HTTP server is running and `GET /health` is requested
- **THEN** the system returns `200 OK` with body `{"status": "ok"}`

### Requirement: Readiness endpoint

The system SHALL expose `GET /ready` returning 200 OK when the pipeline is ready to process messages, 503 otherwise.

#### Scenario: Ready when running

- **WHEN** pipeline is in Running state and `GET /ready` is requested
- **THEN** the system returns `200 OK` with body `{"ready": true}`

#### Scenario: Not ready when draining

- **WHEN** pipeline is draining and `GET /ready` is requested
- **THEN** the system returns `503 Service Unavailable` with body `{"ready": false, "reason": "draining"}`

### Requirement: Pipeline status endpoint

The system SHALL expose `GET /api/v1/pipeline` returning current pipeline status as JSON.

#### Scenario: Get pipeline status

- **WHEN** `GET /api/v1/pipeline` is requested
- **THEN** the system returns `200 OK` with JSON containing name, state, uptime, and summary metrics

### Requirement: List nodes endpoint

The system SHALL expose `GET /api/v1/nodes` returning list of all nodes as JSON array.

#### Scenario: List all nodes

- **WHEN** `GET /api/v1/nodes` is requested
- **THEN** the system returns `200 OK` with JSON array of node objects (id, type, state, metrics)

### Requirement: Get single node endpoint

The system SHALL expose `GET /api/v1/nodes/:id` returning details for a specific node.

#### Scenario: Get existing node

- **WHEN** `GET /api/v1/nodes/filter` is requested for an existing node
- **THEN** the system returns `200 OK` with JSON containing node details

#### Scenario: Get non-existent node

- **WHEN** `GET /api/v1/nodes/nonexistent` is requested
- **THEN** the system returns `404 Not Found` with error body

### Requirement: Hot-swap endpoint

The system SHALL expose `POST /api/v1/nodes/:id/hot-swap` to trigger hot-swap on a node. The request SHALL block until hot-swap completes.

#### Scenario: Successful hot-swap

- **WHEN** `POST /api/v1/nodes/filter/hot-swap` is requested
- **THEN** the system triggers hot-swap, waits for completion, and returns `200 OK` with timing metrics

#### Scenario: Hot-swap conflict

- **WHEN** `POST /api/v1/nodes/filter/hot-swap` is requested while another swap is in progress
- **THEN** the system returns `409 Conflict` with error body

#### Scenario: Hot-swap non-existent node

- **WHEN** `POST /api/v1/nodes/nonexistent/hot-swap` is requested
- **THEN** the system returns `404 Not Found`

### Requirement: Reload config endpoint

The system SHALL expose `POST /api/v1/pipeline/reload` to reload configuration and hot-swap changed nodes.

#### Scenario: Reload with changes

- **WHEN** `POST /api/v1/pipeline/reload` is requested and config has changes
- **THEN** the system reloads, hot-swaps changed nodes, and returns `200 OK` with list of swapped nodes

#### Scenario: Reload with no changes

- **WHEN** `POST /api/v1/pipeline/reload` is requested and config is unchanged
- **THEN** the system returns `200 OK` with empty swapped_nodes list

### Requirement: Drain endpoint

The system SHALL expose `POST /api/v1/pipeline/drain` to drain the pipeline.

#### Scenario: Drain running pipeline

- **WHEN** `POST /api/v1/pipeline/drain` is requested
- **THEN** the system initiates drain and returns `200 OK` when drain completes

### Requirement: Shutdown endpoint

The system SHALL expose `POST /api/v1/pipeline/shutdown` to gracefully shut down the pipeline.

#### Scenario: Shutdown pipeline

- **WHEN** `POST /api/v1/pipeline/shutdown` is requested
- **THEN** the system drains, shuts down, and returns `200 OK` (connection may close before response)

### Requirement: Metrics endpoint

The system SHALL expose `GET /metrics` returning metrics in Prometheus text format.

#### Scenario: Get metrics

- **WHEN** `GET /metrics` is requested
- **THEN** the system returns `200 OK` with `Content-Type: text/plain` body in Prometheus exposition format

### Requirement: Configurable bind address

The system SHALL read API bind address from configuration. Default SHALL be `127.0.0.1:9090`.

#### Scenario: Default bind

- **WHEN** no `[api]` section in config
- **THEN** API server binds to `127.0.0.1:9090`

#### Scenario: Custom bind

- **WHEN** config contains `[api] bind = "0.0.0.0:8080"`
- **THEN** API server binds to `0.0.0.0:8080`

### Requirement: Optionally separate metrics port

The system SHALL support optionally serving metrics on a separate port from the control API.

#### Scenario: Same port (default)

- **WHEN** `[metrics] bind` is not set or equals `[api] bind`
- **THEN** metrics are served on the same server as the control API

#### Scenario: Separate port

- **WHEN** config contains `[api] bind = "127.0.0.1:9090"` and `[metrics] bind = "0.0.0.0:9091"`
- **THEN** control API is on port 9090 (localhost) and metrics on port 9091 (all interfaces)

### Requirement: Consistent error responses

The system SHALL return consistent JSON error responses for all error conditions.

#### Scenario: Error response format

- **WHEN** any error occurs
- **THEN** the system returns appropriate HTTP status with body `{"error": {"code": "...", "message": "...", "details": {...}}}`
