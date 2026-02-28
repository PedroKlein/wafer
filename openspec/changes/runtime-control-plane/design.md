## Context

WAFER runtime currently runs as a monolithic binary with no external control interface. The hot-swap mechanism (ADR-0003) needs a trigger, and observability requires metrics export. Two deployment scenarios must be supported:

1. **Standalone**: Runtime as a binary managed by systemd/Docker/K8s, controlled via HTTP API
2. **Embedded**: Runtime as a library linked into C/C++ applications via ABI calls

**Current state:**
- Single-crate project (`wafer-poc`)
- No HTTP server
- No external control interface
- Internal metrics collection exists but isn't exposed

**Constraints:**
- Core runtime must remain minimal for C ABI embedding
- HTTP API must be optional (compile-time feature flag)
- No custom supervisor—orchestration delegated to external tools
- Localhost-only API for thesis scope (no auth)

## Goals / Non-Goals

**Goals:**
- `PipelineControl` trait as the stable ABI surface for all control operations
- HTTP REST API as an optional feature wrapping the control trait
- `waferctl` CLI for human-friendly management of one or more runtime instances
- Prometheus-compatible metrics endpoint
- Event subscription for observability (hot-swap progress, state changes)
- Design that supports future K8s operator consumption

**Non-Goals:**
- Custom supervisor process (systemd/K8s handles multi-pipeline)
- Authentication/authorization (thesis scope: localhost only)
- WebSocket/SSE streaming (polling is sufficient)
- Pipeline-to-pipeline communication (future work)
- Config file watching (explicit `reload_config()` instead)

## Decisions

### D1: Crate Architecture - Library-First Split

Split into workspace with four crates:

```
wafer-poc/
├── Cargo.toml              # Workspace manifest
└── crates/
    ├── wafer-core/         # Library: pipeline execution + PipelineControl trait
    ├── wafer-runtime/      # Binary: thin wrapper, enables http-api feature
    ├── wafer-types/        # Library: shared API types
    └── waferctl/           # Binary: HTTP client CLI
```

**Rationale:**
- `wafer-core` is the C-ABI-ready library—no HTTP, no CLI, just core functionality
- `wafer-runtime` is a thin binary that enables HTTP by default
- Clean separation allows independent evolution and testing
- Future C ABI exports only need `wafer-core`

**Alternatives considered:**
- Single binary with runtime flags → Bloats core, complicates C ABI story
- Separate processes communicating via IPC → Adds complexity without benefit for single-pipeline case

### D2: Control Interface - Async Trait

Define `PipelineControl` as an async trait in wafer-core:

```rust
pub trait PipelineControl: Send + Sync {
    async fn hot_swap(&self, node_id: &str) -> Result<HotSwapResult, ControlError>;
    async fn reload_config(&self) -> Result<ReloadResult, ControlError>;
    async fn drain(&self) -> Result<(), ControlError>;
    async fn shutdown(&self) -> Result<(), ControlError>;
    fn status(&self) -> PipelineStatus;           // sync, cheap
    fn metrics(&self) -> MetricsSnapshot;         // sync, cheap
    fn nodes(&self) -> Vec<NodeInfo>;             // sync, cheap
    fn subscribe(&self) -> EventReceiver;         // event stream
}
```

**Rationale:**
- Async for potentially slow operations (drain phase can take seconds)
- Sync for cheap queries (status, metrics) to avoid unnecessary overhead
- `subscribe()` enables observability without polling
- Trait allows multiple implementations (direct, HTTP-wrapped, mocked for tests)

**Alternatives considered:**
- All sync with callbacks → Awkward in async Rust ecosystem
- Channel-based command pattern → More complex, less intuitive API

### D3: HTTP API - Feature-Flagged Axum

Use Axum with feature flag `http-api`:

```toml
# wafer-core/Cargo.toml
[features]
default = []
http-api = ["axum", "tower-http", "prometheus-client"]
```

```rust
// wafer-runtime enables it by default
wafer-core = { path = "../wafer-core", features = ["http-api"] }
```

**Rationale:**
- Axum is async-native, Tokio-integrated, lightweight (~500 lines of wrapper code)
- Feature flag keeps core minimal when HTTP isn't needed
- `wafer-runtime` enables feature by default; embedded users disable it

**Alternatives considered:**
- Unix domain sockets → Adds complexity, no clear benefit over HTTP for our use case
- gRPC → Overkill, adds protobuf dependency
- Always include HTTP → Bloats embedded use case unnecessarily

### D4: Metrics Port - Optionally Separate

Allow metrics on a separate port from control API:

```toml
[api]
enabled = true
bind = "127.0.0.1:9090"      # Control API (localhost only)

[metrics]
enabled = true
bind = "0.0.0.0:9091"        # Metrics (open for Prometheus)
path = "/metrics"
```

If `metrics.bind` equals `api.bind` (or is unset), metrics served on same server. Otherwise, spawn separate server.

**Rationale:**
- Security: Control API on localhost, metrics open for scraping
- Flexibility: Some setups want unified port, others want separation
- Simple default: Same port unless explicitly configured otherwise

### D5: No Supervisor - Delegate to Orchestrators

Explicitly do NOT build a supervisor process. Multi-pipeline is handled by:

- **Bare metal**: systemd units, one per pipeline
- **Docker**: docker-compose with multiple services
- **Kubernetes**: Deployments + future CRD operator

**Rationale:**
- Avoids reinventing orchestration (systemd, K8s do this well)
- Reduces scope and complexity
- Each runtime is independent—clean failure isolation
- `waferctl` can target multiple endpoints via config

**Alternatives considered:**
- Custom supervisor with unified API → Significant complexity for marginal benefit
- Shared wasmtime engine → Couples pipeline lifecycles, complicates failure handling

### D6: waferctl - Multi-Endpoint Config

waferctl is a stateless HTTP client with config file for convenience:

```toml
# ~/.config/wafer/config.toml
default = "local"

[endpoints.local]
url = "http://127.0.0.1:9090"

[endpoints.pi-gateway]
url = "http://192.168.1.100:9090"

[endpoints.jetson]
url = "http://192.168.1.101:9090"
```

```bash
waferctl status                    # Uses default endpoint
waferctl --endpoint pi-gateway status
waferctl config use jetson         # Change default
```

**Rationale:**
- Simple mental model: waferctl talks to one runtime at a time
- Config file avoids repeating URLs
- `--endpoint` flag for ad-hoc targeting
- No "supervisor mode"—just a collection of named endpoints

### D7: Event System - Broadcast Channel

Use `tokio::sync::broadcast` for pipeline events:

```rust
pub enum PipelineEvent {
    HotSwapStarted { node_id: String },
    HotSwapCompleted { node_id: String, result: HotSwapResult },
    HotSwapFailed { node_id: String, error: String },
    NodeStateChanged { node_id: String, old: NodeState, new: NodeState },
    ConfigReloaded { swapped_nodes: Vec<String> },
    DrainStarted,
    DrainCompleted,
}
```

**Rationale:**
- Broadcast allows multiple subscribers (logging, HTTP streaming, metrics)
- Non-blocking—publishers don't wait for slow subscribers
- Natural fit for observability and debugging

**Note:** HTTP API will NOT expose events via WebSocket/SSE for thesis. Events are for internal use and future extension.

### D8: Concurrency - Single Hot-Swap at a Time

Only one hot-swap operation can be in progress at a time. Concurrent requests return `409 Conflict`.

**Rationale:**
- Simplifies state machine (no need to track multiple concurrent swaps)
- Avoids race conditions in node routing during swap
- Clear mental model for operators
- Can be relaxed in future if needed

### D9: Pipeline Name - Metadata Only

Pipeline name is configuration metadata used for:
- Metrics labels: `wafer_messages_total{pipeline="temp-agg", node="filter"}`
- Log fields: `{"pipeline": "temp-agg", ...}`
- API responses: `GET /api/v1/pipeline` returns `{"name": "temp-agg", ...}`

It does NOT affect execution logic.

**Rationale:**
- Essential for observability when running multiple pipelines
- Zero runtime cost—just a string label
- Optional with sensible default ("default")

## Risks / Trade-offs

| Risk | Impact | Mitigation |
|------|--------|------------|
| HTTP in core adds attack surface | Security | Disabled by default via feature flag, localhost bind |
| Workspace adds development complexity | DX | Clear crate boundaries, documented structure |
| No supervisor limits orchestration | Operations | Document systemd/K8s patterns, provide examples |
| Async trait requires nightly or workaround | Compatibility | Use `async-trait` crate or RPITIT (Rust 1.75+) |
| Feature flags complicate testing | Testing | CI matrix tests both with and without http-api |

**Key Trade-off: Explicit vs Automatic**
- Chose explicit `reload_config()` over file watching
- Users have full control, no surprises from editor temp files
- Slightly higher friction but clearer mental model

**Key Trade-off: No Supervisor**
- Chose delegation to external orchestrators over custom supervisor
- Less scope, simpler architecture
- Requires users to understand systemd/K8s for multi-pipeline
- Can add supervisor later if real need emerges

## Open Questions (Resolved)

- [x] **Q1:** Should `reload_config()` auto-detect changed nodes and hot-swap them, or require explicit node IDs?
  - **Decision:** Auto-detect (compares old vs new config, swaps changed nodes)
  - **Rationale:** More user-friendly, matches kubectl rollout behavior

- [x] **Q2:** What happens if hot-swap fails mid-drain?
  - **Decision:** Option B - Leave in failed state, require manual intervention
  - **Rationale:** Rollback is complex and may not be possible if old WASM was unloaded. Clear error reporting + manual recovery is simpler and more predictable.

- [x] **Q3:** Should metrics include histograms or just counters/gauges?
  - **Decision:** Start with counters/gauges only
  - **Rationale:** Thesis scope. Can add histograms later for latency percentiles if needed.
