# Runtime Control Plane Implementation Tasks

> **Implementation Order:** This change should be implemented first. It provides the crate 
> structure and control interface that other changes depend on.
>
> **Related Changes:**
> - `hot-swap-mechanism` depends on this for triggers (fills in `hot_swap()` stub)
> - `observability-prometheus` depends on this for HTTP transport (fills in `/metrics` handler)

## 1. Project Restructure

- [x] 1.1 Create workspace Cargo.toml at root (convert from single crate)
- [x] 1.2 Create `crates/` directory structure
- [x] 1.3 Move current `src/` to `crates/wafer-core/src/`
- [x] 1.4 Create `crates/wafer-core/Cargo.toml` with `http-api` feature flag
- [x] 1.5 Create `crates/wafer-runtime/` skeleton (binary crate)
- [x] 1.6 Create `crates/wafer-types/` skeleton (shared types)
- [x] 1.7 Create `crates/waferctl/` skeleton (CLI binary)
- [x] 1.8 Update all import paths and verify `cargo build` succeeds
- [ ] 1.9 Update CI scripts for workspace structure
- [x] 1.10 Update justfile commands for new structure

## 2. Shared Types (wafer-types)

- [x] 2.1 Define `PipelineStatus` struct (name, state enum, uptime, summary metrics)
- [x] 2.2 Define `NodeInfo` struct (id, node_type, state, metrics)
- [x] 2.3 Define `NodeState` enum (Running, Draining, Retired)
- [x] 2.4 Define `HotSwapResult` struct (node_id, timing metrics, message counts)
- [x] 2.5 Define `ReloadResult` struct (swapped_nodes list)
- [x] 2.6 Define `ControlError` enum (NodeNotFound, SwapInProgress, NotSwappable, NotImplemented, ConfigError, etc.)
- [x] 2.7 Define `MetricsSnapshot` struct (counters, gauges per node/queue)
- [x] 2.8 Define `PipelineEvent` enum (HotSwapStarted, HotSwapCompleted, etc.)
- [x] 2.9 Define API request/response DTOs (ErrorResponse, etc.)
- [x] 2.10 Add serde Serialize/Deserialize for all types
- [x] 2.11 Write unit tests for serialization round-trips

## 3. PipelineControl Trait (wafer-core)

> **Note:** `hot_swap()` and `reload_config()` implementations here are stubs that return 
> `ControlError::NotImplemented`. Full implementation comes from `hot-swap-mechanism` change.

- [x] 3.1 Define `PipelineControl` trait with async methods
- [x] 3.2 Define `EventReceiver` type alias (tokio broadcast receiver)
- [x] 3.3 Implement `hot_swap()` stub (returns NotImplemented until hot-swap-mechanism)
- [x] 3.4 Implement `reload_config()` stub (returns NotImplemented until hot-swap-mechanism)
- [x] 3.5 Implement `drain()` method
- [x] 3.6 Implement `shutdown()` method
- [x] 3.7 Implement `status()` method (sync, returns PipelineStatus)
- [x] 3.8 Implement `metrics()` method (sync, returns MetricsSnapshot)
- [x] 3.9 Implement `nodes()` method (sync, returns Vec<NodeInfo>)
- [x] 3.10 Implement `subscribe()` method (returns EventReceiver)
- [ ] 3.11 Add swap-in-progress lock (reject concurrent swaps) [deferred: hot-swap-mechanism]
- [ ] 3.12 Wire event broadcast to hot-swap coordinator [deferred: hot-swap-mechanism]
- [x] 3.13 Write unit tests for PipelineControl methods

## 4. HTTP API Server (wafer-core, feature-gated)

- [x] 4.1 Add axum, tower-http, prometheus-client to Cargo.toml (feature-gated)
- [x] 4.2 Create `src/api/mod.rs` with feature gate
- [x] 4.3 Create `src/api/server.rs` with Axum router setup
- [x] 4.4 Implement `GET /health` handler
- [x] 4.5 Implement `GET /ready` handler
- [x] 4.6 Implement `GET /api/v1/pipeline` handler
- [x] 4.7 Implement `GET /api/v1/nodes` handler
- [x] 4.8 Implement `GET /api/v1/nodes/:id` handler
- [x] 4.9 Implement `POST /api/v1/nodes/:id/hot-swap` handler
- [x] 4.10 Implement `POST /api/v1/pipeline/reload` handler
- [x] 4.11 Implement `POST /api/v1/pipeline/drain` handler
- [x] 4.12 Implement `POST /api/v1/pipeline/shutdown` handler
- [x] 4.13 Implement `GET /metrics` handler (Prometheus format)
- [x] 4.14 Add error handling middleware for consistent error responses
- [x] 4.15 Add request tracing middleware (tower-http)
- [ ] 4.16 Write integration tests for all endpoints

## 5. Configuration

- [x] 5.1 Add `ApiConfig` struct to config schema (enabled, bind)
- [x] 5.2 Add `MetricsConfig` struct to config schema (enabled, bind, path)
- [x] 5.3 Add `[api]` section parsing to config loader
- [x] 5.4 Add `[metrics]` section parsing to config loader
- [x] 5.5 Add `--api-bind` and `--no-api` CLI flags to wafer-runtime
- [ ] 5.6 Implement separate metrics server spawn when `metrics.bind != api.bind`
- [x] 5.7 Write tests for config parsing

## 6. wafer-runtime Binary

- [x] 6.1 Create main.rs with CLI argument parsing (clap)
- [x] 6.2 Load config and create wafer-core Runtime
- [x] 6.3 Start HTTP API server when enabled (feature always on in binary)
- [x] 6.4 Start separate metrics server if configured
- [x] 6.5 Wire up signal handlers (SIGTERM, SIGINT) to shutdown
- [x] 6.6 Add graceful shutdown for API server
- [ ] 6.7 Write integration test: start runtime, hit health endpoint

## 7. waferctl CLI Structure

- [x] 7.1 Add clap, reqwest, tabled, serde_json dependencies
- [x] 7.2 Create main.rs with clap command structure
- [x] 7.3 Create `src/client.rs` with `WaferClient` HTTP client
- [x] 7.4 Create `src/config.rs` for endpoint config file handling
- [x] 7.5 Implement endpoint resolution (--endpoint flag > config default > localhost)
- [x] 7.6 Add `--json` global flag for machine-readable output
- [x] 7.7 Create `src/output.rs` for table formatting utilities

## 8. waferctl Commands

- [x] 8.1 Implement `config set-endpoint` command
- [x] 8.2 Implement `config use` command
- [x] 8.3 Implement `config list` command
- [x] 8.4 Implement `health` command
- [x] 8.5 Implement `status` command
- [x] 8.6 Implement `nodes` command (table output)
- [x] 8.7 Implement `node <id>` command
- [x] 8.8 Implement `hot-swap <node-id>` command
- [x] 8.9 Implement `reload` command
- [x] 8.10 Implement `drain` command
- [x] 8.11 Implement `shutdown` command
- [x] 8.12 Implement `metrics` command (human-readable and --raw)
- [x] 8.13 Add --help for all commands
- [x] 8.14 Write tests for command parsing

## 9. Error Handling

- [x] 9.1 Define exit codes (0=success, 1=user error, 2=API error, 3=connection)
- [x] 9.2 Implement error formatting for human output
- [x] 9.3 Implement error formatting for JSON output
- [x] 9.4 Write tests for error scenarios

## 10. Documentation

- [x] 10.1 Update MVP.md with control plane status
- [x] 10.2 Create waferctl README with usage examples
- [x] 10.3 Add API endpoint documentation to SPEC.md or separate API.md
- [x] 10.4 Add example config with [api] and [metrics] sections
- [x] 10.5 Update SPEC.md milestone checkboxes
- [x] 10.6 Create ADR for runtime architecture split decision

## 11. End-to-End Testing

- [ ] 11.1 Write E2E test: waferctl status against running runtime
- [ ] 11.2 Write E2E test: waferctl hot-swap triggers actual swap
- [ ] 11.3 Write E2E test: waferctl reload detects config changes
- [ ] 11.4 Write E2E test: Prometheus scrape of /metrics endpoint
- [ ] 11.5 Test multi-endpoint config with multiple runtimes
