# Control Plane Implementation Tasks

## 1. Project Restructure

- [ ] 1.1 Create workspace Cargo.toml at root
- [ ] 1.2 Move current src/ to crates/wafer-runtime/
- [ ] 1.3 Create crates/wafer-types/ for shared API types
- [ ] 1.4 Create crates/waferctl/ skeleton
- [ ] 1.5 Update all import paths and verify build
- [ ] 1.6 Update CI/CD scripts for workspace structure

## 2. Shared Types (wafer-types)

- [ ] 2.1 Define `PipelineStatus` struct (name, status enum, metrics)
- [ ] 2.2 Define `NodeInfo` struct (id, type, state, metrics)
- [ ] 2.3 Define `SwapResult` struct (timing metrics, message counts)
- [ ] 2.4 Define `ResyncResponse` struct (changes list)
- [ ] 2.5 Define `ErrorResponse` struct (code, message, details)
- [ ] 2.6 Add serde serialization for all types
- [ ] 2.7 Write unit tests for serialization round-trips

## 3. REST API Server (wafer-runtime)

- [ ] 3.1 Add axum, tower-http dependencies
- [ ] 3.2 Create src/api/mod.rs with router setup
- [ ] 3.3 Implement GET /health handler
- [ ] 3.4 Implement GET /ready handler
- [ ] 3.5 Implement GET /live handler
- [ ] 3.6 Implement GET /metrics handler (delegate to existing)
- [ ] 3.7 Create src/api/routes.rs for /api/v1/* routes
- [ ] 3.8 Implement GET /api/v1/pipeline handler
- [ ] 3.9 Implement GET /api/v1/pipeline/config handler
- [ ] 3.10 Implement POST /api/v1/pipeline/resync handler
- [ ] 3.11 Implement POST /api/v1/pipeline/drain handler
- [ ] 3.12 Implement POST /api/v1/pipeline/resume handler
- [ ] 3.13 Implement POST /api/v1/pipeline/stop handler
- [ ] 3.14 Implement GET /api/v1/nodes handler
- [ ] 3.15 Implement GET /api/v1/nodes/:id handler
- [ ] 3.16 Implement GET /api/v1/nodes/:id/metrics handler
- [ ] 3.17 Implement POST /api/v1/nodes/:id/hot-swap handler
- [ ] 3.18 Add error handling middleware for consistent responses
- [ ] 3.19 Add request tracing middleware
- [ ] 3.20 Write integration tests for all endpoints

## 4. API Configuration

- [ ] 4.1 Add ApiConfig struct to config schema
- [ ] 4.2 Add [api] section parsing to config loader
- [ ] 4.3 Add --api-bind CLI flag
- [ ] 4.4 Make API server optional (disabled when not configured)
- [ ] 4.5 Start API server from main.rs when enabled
- [ ] 4.6 Add graceful shutdown for API server
- [ ] 4.7 Write tests for config parsing

## 5. waferctl CLI Structure

- [ ] 5.1 Add clap, reqwest, tabled dependencies
- [ ] 5.2 Create main.rs with clap command structure
- [ ] 5.3 Create src/client.rs with WaferClient struct
- [ ] 5.4 Implement connection URL resolution (flag > env > default)
- [ ] 5.5 Add --json and --quiet global flags
- [ ] 5.6 Create src/output.rs for formatting utilities

## 6. waferctl Commands

- [ ] 6.1 Implement `health` command
- [ ] 6.2 Implement `status` command
- [ ] 6.3 Implement `resync` command
- [ ] 6.4 Implement `hot-swap` command with --wasm flag
- [ ] 6.5 Implement `nodes` command (table output)
- [ ] 6.6 Implement `node <id>` command
- [ ] 6.7 Implement `drain` command
- [ ] 6.8 Implement `resume` command
- [ ] 6.9 Implement `stop` command
- [ ] 6.10 Implement `metrics` command with --json option
- [ ] 6.11 Add --help for all commands
- [ ] 6.12 Write tests for command parsing

## 7. Error Handling

- [ ] 7.1 Define exit codes (0=success, 1=user error, 2=API error, 3=connection)
- [ ] 7.2 Implement error formatting for human output
- [ ] 7.3 Implement error formatting for JSON output
- [ ] 7.4 Write tests for error scenarios

## 8. Integration with Hot-Swap

- [ ] 8.1 Wire resync handler to config diff and hot-swap coordinator
- [ ] 8.2 Wire hot-swap handler to hot-swap coordinator
- [ ] 8.3 Ensure swap-in-progress lock is respected by API
- [ ] 8.4 Return swap metrics in API response
- [ ] 8.5 Write end-to-end test: waferctl resync triggers hot-swap

## 9. Documentation

- [ ] 9.1 Update MVP.md with control plane status
- [ ] 9.2 Create waferctl README with usage examples
- [ ] 9.3 Document API endpoints in SPEC.md or separate API.md
- [ ] 9.4 Add example config with [api] section
- [ ] 9.5 Update SPEC.md milestone checkboxes
