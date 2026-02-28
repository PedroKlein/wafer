# rest-api Specification

## Purpose
TBD - created by archiving change control-plane. Update Purpose after archive.
## Requirements
### Requirement: Health and probe endpoints

The runtime SHALL expose health check endpoints for orchestration compatibility.

#### Scenario: Basic health check
- **WHEN** `GET /health` is called
- **THEN** response is `200 OK` with `{"status": "healthy"}` when runtime is operational
- **THEN** response is `503 Service Unavailable` when runtime is unhealthy

#### Scenario: Kubernetes readiness probe
- **WHEN** `GET /ready` is called
- **THEN** response is `200 OK` when pipeline is running and can accept traffic
- **THEN** response is `503 Service Unavailable` when pipeline is draining or stopped

#### Scenario: Kubernetes liveness probe
- **WHEN** `GET /live` is called
- **THEN** response is `200 OK` when the process is alive
- **THEN** response is `503 Service Unavailable` only if the process should be restarted

### Requirement: Pipeline status endpoint

The runtime SHALL expose pipeline status via REST API.

#### Scenario: Get pipeline status
- **WHEN** `GET /api/v1/pipeline` is called
- **THEN** response includes pipeline name, status (running/draining/stopped), node count, and aggregate metrics
- **THEN** response includes `uptime_seconds` and `messages_processed` counters

#### Scenario: Get running configuration
- **WHEN** `GET /api/v1/pipeline/config` is called
- **THEN** response includes the current running configuration in TOML format

### Requirement: Resync endpoint for hot-swap triggering

The runtime SHALL support explicit config reload and hot-swap via REST API.

#### Scenario: Resync with no changes
- **WHEN** `POST /api/v1/pipeline/resync` is called
- **WHEN** the config file has not changed since last load
- **THEN** response is `200 OK` with empty changes list
- **THEN** no hot-swaps are triggered

#### Scenario: Resync with WASM path changes
- **WHEN** `POST /api/v1/pipeline/resync` is called
- **WHEN** a node's `wasm` path has changed in the config file
- **THEN** hot-swap is triggered for that node
- **THEN** response includes the list of changes with swap results

#### Scenario: Resync with invalid config
- **WHEN** `POST /api/v1/pipeline/resync` is called
- **WHEN** the config file has invalid syntax or semantics
- **THEN** response is `400 Bad Request` with error details
- **THEN** no changes are applied
- **THEN** pipeline continues with previous configuration

### Requirement: Node listing and details endpoints

The runtime SHALL expose node information via REST API.

#### Scenario: List all nodes
- **WHEN** `GET /api/v1/nodes` is called
- **THEN** response includes list of all nodes with id, type, state, and summary metrics

#### Scenario: Get node details
- **WHEN** `GET /api/v1/nodes/:id` is called
- **WHEN** the node exists
- **THEN** response includes full node details, state, configuration, and metrics

#### Scenario: Get non-existent node
- **WHEN** `GET /api/v1/nodes/:id` is called
- **WHEN** the node does not exist
- **THEN** response is `404 Not Found` with error message

### Requirement: Direct hot-swap endpoint

The runtime SHALL support triggering hot-swap for a specific node via REST API.

#### Scenario: Hot-swap with local WASM file
- **WHEN** `POST /api/v1/nodes/:id/hot-swap` is called
- **WHEN** request body contains `{"wasm_path": "/path/to/new.wasm"}`
- **THEN** hot-swap is triggered for the specified node
- **THEN** response includes swap metrics (timing, message counts)

#### Scenario: Hot-swap node not found
- **WHEN** `POST /api/v1/nodes/:id/hot-swap` is called
- **WHEN** the node does not exist
- **THEN** response is `404 Not Found`

#### Scenario: Hot-swap already in progress
- **WHEN** `POST /api/v1/nodes/:id/hot-swap` is called
- **WHEN** a hot-swap is already in progress for that node
- **THEN** response is `409 Conflict` with message indicating swap in progress

#### Scenario: Hot-swap validation failure
- **WHEN** `POST /api/v1/nodes/:id/hot-swap` is called
- **WHEN** the new WASM component fails validation or initialization
- **THEN** response is `400 Bad Request` with validation error details
- **THEN** the original node continues running unchanged

### Requirement: Pipeline control endpoints

The runtime SHALL support graceful drain, resume, and stop operations.

#### Scenario: Drain pipeline
- **WHEN** `POST /api/v1/pipeline/drain` is called
- **THEN** all nodes transition to draining state
- **THEN** new messages are held at source
- **THEN** response is `200 OK` when drain starts

#### Scenario: Resume pipeline
- **WHEN** `POST /api/v1/pipeline/resume` is called
- **WHEN** pipeline is in drained state
- **THEN** all nodes transition back to running state
- **THEN** held messages are released

#### Scenario: Resume when not drained
- **WHEN** `POST /api/v1/pipeline/resume` is called
- **WHEN** pipeline is already running
- **THEN** response is `200 OK` (idempotent, no-op)

#### Scenario: Stop pipeline
- **WHEN** `POST /api/v1/pipeline/stop` is called
- **THEN** pipeline initiates graceful shutdown
- **THEN** drain is performed first
- **THEN** response is `200 OK` with acknowledgment

### Requirement: API server configuration

The REST API server SHALL be configurable and disabled by default.

#### Scenario: API disabled by default
- **WHEN** no `[api]` section is in config
- **THEN** no HTTP server is started
- **THEN** pipeline runs without network exposure

#### Scenario: API enabled via config
- **WHEN** config contains `[api] enabled = true`
- **THEN** HTTP server starts on configured bind address

#### Scenario: API enabled via CLI flag
- **WHEN** `--api-bind <addr>` flag is provided
- **THEN** HTTP server starts on specified address
- **THEN** CLI flag overrides config file setting

### Requirement: Error response format

All error responses SHALL follow a consistent JSON format.

#### Scenario: Error response structure
- **WHEN** any endpoint returns an error
- **THEN** response body is JSON with `error.code`, `error.message`, and optional `error.details`
- **THEN** HTTP status code matches error type (400, 404, 409, 500, 503)

