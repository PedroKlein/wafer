## Why

WAFER needs a control interface for managing pipeline operations (hot-swap, drain, shutdown) and exposing metrics for observability. The runtime must support two deployment modes:

1. **Standalone binary**: HTTP API enabled by default for remote management via `waferctl` or future K8s operators
2. **Embedded library**: Direct ABI calls with optional HTTP, for embedding in C/C++ applications

The key insight: the control interface should be a **trait in wafer-core**, with HTTP being just one transport implementation. This keeps the core minimal while enabling flexible deployment patterns. Multi-pipeline orchestration is explicitly delegated to external tools (systemd, Docker, Kubernetes) rather than building a custom supervisor.

**SPEC Reference:** Sections 7.3 (Configuration Operations), 12 (Observability)

## What Changes

- **wafer-core library**: Extract core runtime into a library crate with `PipelineControl` trait
- **wafer-runtime binary**: Thin wrapper enabling HTTP API by default
- **HTTP API**: REST endpoints for pipeline control and Prometheus metrics (feature-flagged)
- **waferctl CLI**: HTTP client for human-friendly pipeline management
- **wafer-types crate**: Shared API types between wafer-core and waferctl
- **Event system**: Subscribe to pipeline events (hot-swap progress, state changes)
- **Project restructure**: Cargo workspace with multiple crates

**Explicit non-goals:**
- No custom supervisor process (use systemd/K8s/Docker instead)
- No pipeline-to-pipeline communication (future work)
- No authentication (localhost-first for thesis scope)

**Related Changes:**
- `hot-swap-mechanism`: Implements the actual hot-swap logic that `PipelineControl::hot_swap()` delegates to
- `observability-prometheus`: Provides `MetricsRegistry` that the `/metrics` endpoint serves

## Capabilities

### New Capabilities
- `pipeline-control`: Core `PipelineControl` trait with async methods for hot_swap, reload_config, drain, shutdown, status, metrics, nodes, and event subscription
- `control-api`: HTTP REST API exposing PipelineControl operations, with optional separate metrics port
- `waferctl`: CLI tool for pipeline management with multi-endpoint configuration support

### Modified Capabilities
*(none - this is additive, existing pipeline execution is unchanged)*

## Impact

**Code:**
- New crate structure: `crates/wafer-core/`, `crates/wafer-runtime/`, `crates/waferctl/`, `crates/wafer-types/`
- Current `src/` moves into `wafer-core`
- Feature flag `http-api` in wafer-core (optional HTTP server)

**Dependencies:**
- `axum` + `tower-http` (HTTP server, feature-gated)
- `reqwest` (HTTP client for waferctl)
- `tabled` (table output formatting)
- `prometheus-client` (metrics formatting)

**Config:** New optional `[api]` and `[metrics]` sections in pipeline TOML

**WIT:** No changes required

**SPEC Reference:** Section 12.2-12.5 (Observability)
