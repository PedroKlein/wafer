## Why

WAFER needs a control plane for remote management and explicit user control over runtime operations. The core use cases are:

1. **Hot-swap triggering**: Users explicitly request config reload and hot-swap via `waferctl resync` instead of automatic file watching
2. **Remote management**: Raspberry Pi/edge deployments need remote access from development machines
3. **Observability access**: CLI access to metrics without requiring Prometheus setup
4. **Graceful operations**: Controlled drain, resume, and shutdown for maintenance windows
5. **Future K8s operator**: REST API designed to be consumed by a Kubernetes operator

**Design principle**: Explicit over magic. Users control when changes happen, not file system events.

## What Changes

- **REST API server** in wafer-runtime using Axum (optional, disabled by default)
- **waferctl CLI** as a separate binary for interacting with the runtime
- **Shared types crate** for API request/response schemas
- **Project restructure** to Cargo workspace with multiple crates

## Capabilities

### New Capabilities
- `rest-api`: HTTP server with endpoints for health, pipeline status, node management, hot-swap triggers, and pipeline control
- `waferctl`: CLI tool providing human-friendly access to all REST API functionality

### Modified Capabilities
- `hot-swap-triggers` (from hot-swap-mechanism): Remove file watch, add REST endpoint trigger

## Impact

**Code:**
- `crates/wafer-runtime/` - Move current `src/` here, add `src/api/` module
- `crates/waferctl/` - New CLI crate
- `crates/wafer-types/` - New shared types crate
- Root `Cargo.toml` becomes workspace manifest

**Dependencies:**
- `axum` - HTTP server (async, Tokio-native)
- `tower-http` - Middleware (tracing, CORS)
- `reqwest` - HTTP client for waferctl
- `tabled` - Table output formatting

**Config:** New optional section:
```toml
[api]
enabled = true
bind = "127.0.0.1:9090"
```

**SPEC Reference:** Sections 7.3 (Configuration Operations), 9.2 (Change Triggers), 12.2-12.5 (Observability)
