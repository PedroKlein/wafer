# WAFER Control Plane API

This directory contains API documentation for the WAFER runtime HTTP control plane.

## Files

- **`openapi.yaml`** - OpenAPI 3.1 specification for all endpoints
- **`bruno-collection/`** - Bruno collection for local API testing

## API Overview

The control plane provides:

| Endpoint | Method | Description |
|----------|--------|-------------|
| `/health` | GET | Liveness probe (always 200 if server up) |
| `/ready` | GET | Readiness probe (200 when pipeline running) |
| `/api/v1/pipeline` | GET | Get pipeline status and metrics |
| `/api/v1/pipeline/reload` | POST | Reload configuration and hot-swap changed nodes |
| `/api/v1/pipeline/drain` | POST | Graceful shutdown |
| `/api/v1/pipeline/shutdown` | POST | Immediate shutdown |
| `/api/v1/nodes` | GET | List all nodes |
| `/api/v1/nodes/{id}` | GET | Get node details |
| `/api/v1/nodes/{id}/hot-swap` | POST | Hot-swap a node (drain-and-flip) |
| `/metrics` | GET | Prometheus metrics |

## Default Ports

- **API Server**: `localhost:9090`
- **Metrics Server**: `localhost:9091` (when configured separately)

## Using the Bruno Collection

[Bruno](https://www.usebruno.com/) is a fast, git-friendly API client.

### Setup

1. Install Bruno: `brew install bruno` (macOS) or download from [usebruno.com](https://www.usebruno.com/)
2. Open Bruno and select "Open Collection"
3. Navigate to `docs/api/bruno-collection/`

### Environment

The collection includes a `local` environment with:
- `baseUrl`: `http://localhost:9090` (API server)
- `metricsUrl`: `http://localhost:9091` (Metrics server)

### Running Requests

1. Start a WAFER pipeline with the API enabled:
   ```bash
   wafer-runtime --config examples/simple-pipeline.yaml
   ```

2. In Bruno, select a request (e.g., "Health Check")
3. Click "Send" to execute

### Requests Included

**Health/**
- `health.bru` - Liveness check
- `ready.bru` - Readiness check

**Pipeline/**
- `get-status.bru` - Get pipeline status
- `reload.bru` - Reload configuration
- `drain.bru` - Graceful shutdown
- `shutdown.bru` - Immediate shutdown

**Nodes/**
- `list-nodes.bru` - List all nodes
- `get-node.bru` - Get specific node details
- `hot-swap.bru` - Trigger hot-swap

**Metrics/**
- `get-metrics.bru` - Prometheus metrics

## OpenAPI Specification

The `openapi.yaml` file can be used with:
- **Swagger UI**: Visualize and interact with the API
- **Code generators**: Generate client SDKs
- **Validation**: Validate API responses

### Viewing in Swagger UI

```bash
# Using Docker
docker run -p 8080:8080 -e SWAGGER_JSON=/api/openapi.yaml \
  -v $(pwd)/docs/api:/api swaggerapi/swagger-ui

# Then open http://localhost:8080
```

## Configuration

Enable the API in your pipeline configuration:

```yaml
# pipeline.yaml
api:
  bind: "0.0.0.0:9090"

metrics:
  bind: "0.0.0.0:9091"  # Optional: separate metrics port
```

Or via CLI:
```bash
wafer-runtime --config pipeline.yaml --api-bind 0.0.0.0:9090
```
