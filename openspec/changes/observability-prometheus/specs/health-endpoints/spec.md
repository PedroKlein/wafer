# Health Endpoints

HTTP endpoints for health, readiness, and liveness checks.

**SPEC Reference:** Section 12.5

## ADDED Requirements

### Requirement: Liveness endpoint

The runtime SHALL expose a `/live` endpoint indicating the process is running.

#### Scenario: Process is alive
- **WHEN** HTTP GET request is made to `/live`
- **THEN** response status is 200 OK
- **THEN** response body is `{"status": "ok"}`

### Requirement: Readiness endpoint

The runtime SHALL expose a `/ready` endpoint indicating the pipeline is ready to process messages.

#### Scenario: Pipeline ready
- **WHEN** HTTP GET request is made to `/ready`
- **WHEN** all nodes have completed initialization
- **THEN** response status is 200 OK
- **THEN** response body is `{"status": "ready"}`

#### Scenario: Pipeline not ready
- **WHEN** HTTP GET request is made to `/ready`
- **WHEN** one or more nodes are still initializing
- **THEN** response status is 503 Service Unavailable
- **THEN** response body indicates which nodes are not ready

### Requirement: Health endpoint

The runtime SHALL expose a `/health` endpoint indicating overall system health.

#### Scenario: System healthy
- **WHEN** HTTP GET request is made to `/health`
- **WHEN** pipeline is running normally
- **WHEN** no nodes are in error state
- **THEN** response status is 200 OK
- **THEN** response body is `{"status": "healthy"}`

#### Scenario: System unhealthy
- **WHEN** HTTP GET request is made to `/health`
- **WHEN** one or more nodes are in error state
- **THEN** response status is 503 Service Unavailable
- **THEN** response body includes error details

#### Scenario: System degraded
- **WHEN** HTTP GET request is made to `/health`
- **WHEN** pipeline is running but with warnings (e.g., high queue depth)
- **THEN** response status is 200 OK
- **THEN** response body is `{"status": "degraded", "warnings": [...]}`
