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
| `/api/v1/pipeline/shutdown` | POST | Graceful shutdown |
| `/api/v1/nodes` | GET | List all nodes |
| `/api/v1/nodes/{id}` | GET | Get node details |
| `/api/v1/nodes/{id}/hot-swap` | POST | Hot-swap a Wasm node (watch-channel, between messages) |
| `/metrics` | GET | Prometheus metrics |

See [`docs/interfaces/http-api.md`](../interfaces/http-api.md) for the authoritative reference (request/response schemas, error codes).

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

> **Current runtime caveat.** `examples/dag-passthrough-with-api.toml` uses the
> config syntax accepted by the current runtime binary, but the binary does not
> yet launch the API / metrics servers (gap
> [`A2`](../status/implementation-gaps.md#a2--http-control-plane-never-launched-by-runtime-binary-)).
> The Bruno collection is therefore a contract/testing aid until A2 is closed.

1. Start a WAFER pipeline with the API config enabled:
   ```bash
   wafer-runtime --config examples/dag-passthrough-with-api.toml
   ```

2. After A2 is closed, select a Bruno request (e.g., "Health Check")
3. Click "Send" to execute

### Requests Included

**Health/**
- `health.bru` - Liveness check
- `ready.bru` - Readiness check

**Pipeline/**
- `shutdown.bru` — Graceful shutdown via `POST /api/v1/pipeline/shutdown`

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

Current runtime config syntax:

```toml
# See examples/dag-passthrough-with-api.toml.
# Parses today, but server launch is blocked by A2.
[api]
enabled = true
bind = "127.0.0.1:9090"

[metrics]
enabled = true
bind = "127.0.0.1:9091"
```

Equivalent CLI override for the API bind address:
```bash
wafer-runtime --config examples/dag-passthrough-with-api.toml --api-bind 127.0.0.1:9090
```

For the target operator-facing schema after runtime-migration closes A1, see
[`../interfaces/config-schema.md`](../interfaces/config-schema.md).
