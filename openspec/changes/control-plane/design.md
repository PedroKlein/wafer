## Context

WAFER runtime currently runs as a standalone process with no external control interface. Hot-swap was originally planned with file watching as the trigger, but this approach has drawbacks:

1. **Magic behavior** - Automatic reloads can surprise users
2. **Editor artifacts** - Temp files, swap files trigger false reloads
3. **Race conditions** - Multiple rapid saves cause duplicate swaps
4. **No remote access** - Can't manage edge devices from development machine

**Decision**: Replace file watching with explicit REST API control and a `waferctl` CLI tool.

**Current state:**
- Single-crate project (`wafer-poc`)
- No HTTP server
- No external control interface

## Goals / Non-Goals

**Goals:**
- REST API for pipeline and node management
- `waferctl` CLI for human-friendly interaction
- Design API for future Kubernetes operator consumption
- Localhost-only by default (security)
- Human-readable output with JSON option for scripting

**Non-Goals:**
- Authentication/authorization (thesis scope: localhost only)
- WebSocket/SSE for real-time updates (pull model is sufficient)
- Multi-pipeline management (single pipeline per process)
- Config file watching (explicitly rejected)

## Decisions

### D1: HTTP Framework - Axum

Use Axum for the REST API server:

```rust
let app = Router::new()
    .route("/health", get(health_handler))
    .route("/ready", get(ready_handler))
    .route("/live", get(live_handler))
    .route("/metrics", get(metrics_handler))
    .nest("/api/v1", api_routes())
    .layer(TraceLayer::new_for_http());
```

**Rationale:** Axum is async-native, Tokio-integrated, lightweight, and maintained by the Tokio team. The type-safe extractor pattern reduces runtime errors.

**Alternatives considered:**
- `actix-web`: Different async runtime, adds complexity
- `warp`: Similar but more complex API surface
- Raw `hyper`: Too low-level for our needs

### D2: Project Structure - Cargo Workspace

Restructure to workspace with three crates:

```
wafer-poc/
├── Cargo.toml              # Workspace manifest
├── crates/
│   ├── wafer-runtime/      # Main runtime binary
│   ├── waferctl/           # CLI binary
│   └── wafer-types/        # Shared types library
```

**Rationale:** 
- Separate binaries for runtime and CLI
- Shared types avoid duplication and ensure API compatibility
- Independent compilation speeds up development

### D3: API Design - Resource-Oriented REST

Design REST API with Kubernetes operator consumption in mind:

```
/api/v1/pipeline           # Pipeline resource (singleton)
/api/v1/pipeline/resync    # Action on pipeline
/api/v1/nodes              # Node collection
/api/v1/nodes/:id          # Node resource
/api/v1/nodes/:id/hot-swap # Action on node
```

**Rationale:** Resource-oriented design maps naturally to Kubernetes CRDs. Actions as sub-paths (not verbs in URL) is idiomatic REST.

### D4: Error Response Format

Consistent error responses across all endpoints:

```json
{
  "error": {
    "code": "NODE_NOT_FOUND",
    "message": "Node 'filter' does not exist",
    "details": { "available_nodes": ["source", "transform", "sink"] }
  }
}
```

HTTP status codes:
- `200 OK` - Success
- `400 Bad Request` - Invalid input
- `404 Not Found` - Resource not found
- `409 Conflict` - Operation conflict (e.g., swap already in progress)
- `500 Internal Server Error` - Unexpected error
- `503 Service Unavailable` - Not ready (draining, shutting down)

### D5: waferctl HTTP Client

Use `reqwest` with async runtime:

```rust
pub struct WaferClient {
    base_url: Url,
    client: reqwest::Client,
}

impl WaferClient {
    pub async fn health(&self) -> Result<HealthStatus> { ... }
    pub async fn resync(&self) -> Result<ResyncResponse> { ... }
    pub async fn hot_swap(&self, node_id: &str, wasm: &str) -> Result<SwapResult> { ... }
}
```

**Rationale:** `reqwest` is the de facto HTTP client for Rust, async-native, well-maintained.

### D6: Output Formatting

Default to human-readable tables, `--json` for machine parsing:

```bash
$ waferctl nodes
ID        TYPE       STATE     PROCESSED  LAST_MS
source    source     running      50000      -
filter    transform  running      50000     1.2
sink      sink       running      50000      -

$ waferctl nodes --json
{"nodes":[{"id":"source","type":"source",...}]}
```

**Rationale:** Interactive use should be pleasant; scripts use `--json` and `jq`.

### D7: Configuration

API server is opt-in via config:

```toml
[api]
enabled = true
bind = "127.0.0.1:9090"  # localhost only by default
```

Or via CLI flag:
```bash
wafer run pipeline.toml --api-bind 0.0.0.0:9090
```

**Rationale:** Disabled by default minimizes attack surface. Explicit bind address gives user control.

## Risks / Trade-offs

| Risk | Impact | Mitigation |
|------|--------|------------|
| API adds attack surface | Security | Disabled by default, localhost bind |
| Workspace adds complexity | Development | Clear crate boundaries, shared types |
| reqwest adds binary size | Deployment | ~300KB, acceptable for CLI tool |
| API design locks in | Flexibility | v1 prefix allows future versions |

**Trade-off: Explicit vs Automatic**
- Chose explicit `waferctl resync` over automatic file watch
- Users have full control, no surprises
- Slightly higher friction but much clearer mental model

## Open Questions

- [ ] **Q1:** Should waferctl support config file path for `resync` (send new config to runtime)?
  - Current design: runtime re-reads its own config file
  - Alternative: `waferctl resync --config new.toml` sends config content
  
- [ ] **Q2:** Should there be a `waferctl watch` command that polls and displays status?
  - Could be useful for development
  - Adds complexity, may not be thesis-critical
