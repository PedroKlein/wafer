# Runtime Control Plane Implementation Tasks

> **Implementation Order:** This change should be implemented first. It provides the crate 
> structure and control interface that other changes depend on.
>
> **Related Changes:**
> - `hot-swap-mechanism` depends on this for triggers (fills in `hot_swap()` stub)
> - `observability-prometheus` depends on this for HTTP transport (fills in `/metrics` handler)

## 1. Project Restructure

- [ ] 1.1 Create workspace Cargo.toml at root (convert from single crate)
- [ ] 1.2 Create `crates/` directory structure
- [ ] 1.3 Move current `src/` to `crates/wafer-core/src/`
- [ ] 1.4 Create `crates/wafer-core/Cargo.toml` with `http-api` feature flag
- [ ] 1.5 Create `crates/wafer-runtime/` skeleton (binary crate)
- [ ] 1.6 Create `crates/wafer-types/` skeleton (shared types)
- [ ] 1.7 Create `crates/waferctl/` skeleton (CLI binary)
- [ ] 1.8 Update all import paths and verify `cargo build` succeeds
- [ ] 1.9 Update CI scripts for workspace structure
- [ ] 1.10 Update justfile commands for new structure

## 2. Shared Types (wafer-types)

- [ ] 2.1 Define `PipelineStatus` struct (name, state enum, uptime, summary metrics)
- [ ] 2.2 Define `NodeInfo` struct (id, node_type, state, metrics)
- [ ] 2.3 Define `NodeState` enum (Running, Draining, Retired)
- [ ] 2.4 Define `HotSwapResult` struct (node_id, timing metrics, message counts)
- [ ] 2.5 Define `ReloadResult` struct (swapped_nodes list)
- [ ] 2.6 Define `ControlError` enum (NodeNotFound, SwapInProgress, NotSwappable, NotImplemented, ConfigError, etc.)
- [ ] 2.7 Define `MetricsSnapshot` struct (counters, gauges per node/queue)
- [ ] 2.8 Define `PipelineEvent` enum (HotSwapStarted, HotSwapCompleted, etc.)
- [ ] 2.9 Define API request/response DTOs (ErrorResponse, etc.)
- [ ] 2.10 Add serde Serialize/Deserialize for all types
- [ ] 2.11 Write unit tests for serialization round-trips

## 3. PipelineControl Trait (wafer-core)

> **Note:** `hot_swap()` and `reload_config()` implementations here are stubs that return 
> `ControlError::NotImplemented`. Full implementation comes from `hot-swap-mechanism` change.

- [ ] 3.1 Define `PipelineControl` trait with async methods
- [ ] 3.2 Define `EventReceiver` type alias (tokio broadcast receiver)
- [ ] 3.3 Implement `hot_swap()` stub (returns NotImplemented until hot-swap-mechanism)
- [ ] 3.4 Implement `reload_config()` stub (returns NotImplemented until hot-swap-mechanism)
- [ ] 3.5 Implement `drain()` method
- [ ] 3.6 Implement `shutdown()` method
- [ ] 3.7 Implement `status()` method (sync, returns PipelineStatus)
- [ ] 3.8 Implement `metrics()` method (sync, returns MetricsSnapshot)
- [ ] 3.9 Implement `nodes()` method (sync, returns Vec<NodeInfo>)
- [ ] 3.10 Implement `subscribe()` method (returns EventReceiver)
- [ ] 3.11 Add swap-in-progress lock (reject concurrent swaps)
- [ ] 3.12 Wire event broadcast to hot-swap coordinator
- [ ] 3.13 Write unit tests for PipelineControl methods

## 4. HTTP API Server (wafer-core, feature-gated)

- [ ] 4.1 Add axum, tower-http, prometheus-client to Cargo.toml (feature-gated)
- [ ] 4.2 Create `src/api/mod.rs` with feature gate
- [ ] 4.3 Create `src/api/server.rs` with Axum router setup
- [ ] 4.4 Implement `GET /health` handler
- [ ] 4.5 Implement `GET /ready` handler
- [ ] 4.6 Implement `GET /api/v1/pipeline` handler
- [ ] 4.7 Implement `GET /api/v1/nodes` handler
- [ ] 4.8 Implement `GET /api/v1/nodes/:id` handler
- [ ] 4.9 Implement `POST /api/v1/nodes/:id/hot-swap` handler
- [ ] 4.10 Implement `POST /api/v1/pipeline/reload` handler
- [ ] 4.11 Implement `POST /api/v1/pipeline/drain` handler
- [ ] 4.12 Implement `POST /api/v1/pipeline/shutdown` handler
- [ ] 4.13 Implement `GET /metrics` handler (Prometheus format)
- [ ] 4.14 Add error handling middleware for consistent error responses
- [ ] 4.15 Add request tracing middleware (tower-http)
- [ ] 4.16 Write integration tests for all endpoints

## 5. Configuration

- [ ] 5.1 Add `ApiConfig` struct to config schema (enabled, bind)
- [ ] 5.2 Add `MetricsConfig` struct to config schema (enabled, bind, path)
- [ ] 5.3 Add `[api]` section parsing to config loader
- [ ] 5.4 Add `[metrics]` section parsing to config loader
- [ ] 5.5 Add `--api-bind` and `--no-api` CLI flags to wafer-runtime
- [ ] 5.6 Implement separate metrics server spawn when `metrics.bind != api.bind`
- [ ] 5.7 Write tests for config parsing

## 6. wafer-runtime Binary

- [ ] 6.1 Create main.rs with CLI argument parsing (clap)
- [ ] 6.2 Load config and create wafer-core Runtime
- [ ] 6.3 Start HTTP API server when enabled (feature always on in binary)
- [ ] 6.4 Start separate metrics server if configured
- [ ] 6.5 Wire up signal handlers (SIGTERM, SIGINT) to shutdown
- [ ] 6.6 Add graceful shutdown for API server
- [ ] 6.7 Write integration test: start runtime, hit health endpoint

## 7. waferctl CLI Structure

- [ ] 7.1 Add clap, reqwest, tabled, serde_json dependencies
- [ ] 7.2 Create main.rs with clap command structure
- [ ] 7.3 Create `src/client.rs` with `WaferClient` HTTP client
- [ ] 7.4 Create `src/config.rs` for endpoint config file handling
- [ ] 7.5 Implement endpoint resolution (--endpoint flag > config default > localhost)
- [ ] 7.6 Add `--json` global flag for machine-readable output
- [ ] 7.7 Create `src/output.rs` for table formatting utilities

## 8. waferctl Commands

- [ ] 8.1 Implement `config set-endpoint` command
- [ ] 8.2 Implement `config use` command
- [ ] 8.3 Implement `config list` command
- [ ] 8.4 Implement `health` command
- [ ] 8.5 Implement `status` command
- [ ] 8.6 Implement `nodes` command (table output)
- [ ] 8.7 Implement `node <id>` command
- [ ] 8.8 Implement `hot-swap <node-id>` command
- [ ] 8.9 Implement `reload` command
- [ ] 8.10 Implement `drain` command
- [ ] 8.11 Implement `shutdown` command
- [ ] 8.12 Implement `metrics` command (human-readable and --raw)
- [ ] 8.13 Add --help for all commands
- [ ] 8.14 Write tests for command parsing

## 9. Error Handling

- [ ] 9.1 Define exit codes (0=success, 1=user error, 2=API error, 3=connection)
- [ ] 9.2 Implement error formatting for human output
- [ ] 9.3 Implement error formatting for JSON output
- [ ] 9.4 Write tests for error scenarios

## 10. Documentation

- [ ] 10.1 Update MVP.md with control plane status
- [ ] 10.2 Create waferctl README with usage examples
- [ ] 10.3 Add API endpoint documentation to SPEC.md or separate API.md
- [ ] 10.4 Add example config with [api] and [metrics] sections
- [ ] 10.5 Update SPEC.md milestone checkboxes
- [ ] 10.6 Create ADR for runtime architecture split decision

## 11. End-to-End Testing

- [ ] 11.1 Write E2E test: waferctl status against running runtime
- [ ] 11.2 Write E2E test: waferctl hot-swap triggers actual swap
- [ ] 11.3 Write E2E test: waferctl reload detects config changes
- [ ] 11.4 Write E2E test: Prometheus scrape of /metrics endpoint
- [ ] 11.5 Test multi-endpoint config with multiple runtimes
