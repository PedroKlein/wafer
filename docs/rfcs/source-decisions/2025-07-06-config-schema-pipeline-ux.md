# Config File Schema & Pipeline UX — Session 4 Decisions

**Date:** 2025-07-06  
**Status:** Decided (pending implementation)  
**Scope:** Phase 0 structural refactor — pipeline.toml schema redesign  
**Depends on:** Sessions 1–3 (WIT contracts, host runtime, node type architecture)  
**Feeds into:** Implementation task graph, config crate rewrite  

---

## Context

With WIT contracts (Session 1), host runtime (Session 2), and node type architecture (Session 3) decided, the pipeline configuration schema must be redesigned to match. The current schema has:
- `NodeType::Joiner` (removed in Session 3)
- No `NodeType::Filter` (added in Session 1)
- No error policy config (added in Session 2)
- Untyped `toml::Value` for node config (no type safety)
- Duplicate `Config` / `DagConfig` types
- String-based source/sink types without enum validation

This session redesigns the complete `pipeline.toml` schema.

---

## Decision 1: Node Schema — Map-Keyed with Internally-Tagged Type Dispatch

**Decision:** Switch from `[[nodes]]` array to `[nodes.NAME]` map. The node ID is the TOML key. `type` field dispatches via serde internally-tagged enum.

**Rationale:** Map keys prevent duplicate IDs at parse time. Eliminates the `id` field (DRY). Serde `#[serde(tag = "type")]` gives compile-time exhaustive dispatch.

**TOML:**
```toml
[nodes.mqtt-source]
type = "source"
kind = "mqtt"
broker = "localhost"
port = 1883
topic = "sensors/temperature"

[nodes.threshold-filter]
type = "filter"
plugin = "plugins/threshold-filter.wasm"

[nodes.normalize]
type = "transform"
plugin = "ghcr.io/org/normalize:1.0.0"

[nodes.content-router]
type = "router"
plugin = "plugins/content-router.wasm"

[nodes.output]
type = "sink"
kind = "stdout"
```

**Rust:**
```rust
#[derive(Debug, Clone, Deserialize)]
#[serde(tag = "type", rename_all = "kebab-case")]
pub enum NodeDef {
    Source(SourceDef),
    Sink(SinkDef),
    Transform(WasmNodeDef),
    Filter(WasmNodeDef),
    Router(WasmNodeDef),
}
```

---

## Decision 2: Plugin Reference — Single `plugin` Field with Auto-Detection

**Decision:** Unified `plugin` field on all Wasm nodes. Local paths (no `registry/repo:tag` pattern) vs OCI references (contains registry hostname + `:tag`) are auto-detected using existing `OciReference::parse()`.

**Rationale:** One field, no mutual-exclusion validation needed. Docker CLI uses the same pattern. Unambiguous: local paths never match `registry/repo:tag` format.

**TOML:**
```toml
# Local path
plugin = "plugins/my-filter.wasm"

# OCI reference (auto-detected)
plugin = "ghcr.io/pedroklein/wafer-uppercase:0.0.1"
```

**Rust:**
```rust
impl WasmNodeDef {
    pub fn plugin_source(&self) -> Result<PluginSource> {
        match OciReference::parse(&self.plugin) {
            Some(oci_ref) => Ok(PluginSource::Oci(oci_ref)),
            None => Ok(PluginSource::Local(PathBuf::from(&self.plugin))),
        }
    }
}
```

---

## Decision 3: Merge Topology — Implicit, No Config Required

**Decision:** Multiple edges pointing to the same downstream node IS merge. No explicit merge node or merge config in TOML. Semantic validator checks: only Transform and Sink nodes may have multiple inbound edges.

**Rationale:** Session 3 decided merge is tokio mpsc multi-sender (zero-cost topology). Per-input-edge configuration is already handled by edge-level `capacity` and `overflow` fields. No additional merge-specific config is needed for thesis scope.

**Restriction:** Filter and Router must have exactly one inbound edge (borrow-only semantics require single-stream input).

**TOML:**
```toml
# Implicit merge: two edges to same node
[[edges]]
from = "transform-a"
to = "output-sink"

[[edges]]
from = "transform-b"
to = "output-sink"
```

---

## Decision 4: Error Policy — Pipeline-Level Defaults + Per-Node Override

**Decision:** Top-level `[error_policy]` sets defaults for ALL Wasm nodes. Per-node `[nodes.X.error_policy]` overrides specific categories. Only applies to Wasm nodes (Transform, Filter, Router). Sources/sinks have their own error handling.

**Rationale:** Most pipelines want uniform error handling. Per-node overrides are the exception. CSS-style cascade (global → specific) is well-understood.

**TOML:**
```toml
# Pipeline-level defaults
[error_policy]
bad_input = "dlq"
dependency_failed = { retries = 3, backoff_ms = 200, exhausted = "dlq" }
processing_failed = { retries = 2, backoff_ms = 100, exhausted = "dlq" }
timed_out = "skip"

# Per-node override (only what differs)
[nodes.critical-transform.error_policy]
dependency_failed = { retries = 10, backoff_ms = 500, exhausted = "dlq" }
```

**Defaults (when `[error_policy]` is omitted entirely):**

| Category | Default |
|----------|---------|
| `bad_input` | `"dlq"` |
| `dependency_failed` | `{ retries = 3, backoff_ms = 100, exhausted = "dlq" }` |
| `processing_failed` | `{ retries = 2, backoff_ms = 100, exhausted = "dlq" }` |
| `timed_out` | `"skip"` |
| `unrecoverable` | Always teardown (not configurable) |

**Rust:**
```rust
#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct ErrorPolicyConfig {
    pub bad_input: SimpleAction,
    pub dependency_failed: RetryConfig,
    pub processing_failed: RetryConfig,
    pub timed_out: SimpleAction,
    // unrecoverable is always teardown — not in config
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum SimpleAction { Skip, Dlq }

#[derive(Debug, Clone, Deserialize)]
pub struct RetryConfig {
    pub retries: u32,
    pub backoff_ms: u64,
    pub exhausted: SimpleAction,
}
```

---

## Decision 5: Fuel & Epoch Configuration

**Decision:** Both fuel AND epoch are configured. `[engine]` holds epoch settings and per-type fuel defaults. Individual nodes override with a `fuel` field.

**Rationale:** Fuel = deterministic instruction budget (catches infinite loops). Epoch = wall-clock timeout (catches hangs on host calls). Both serve different purposes. Per-type defaults (Session 3 D10): Transform 10M, Filter/Router 500K.

**TOML:**
```toml
[engine]
epoch_deadline = 100      # epochs before interrupt
epoch_tick_ms = 10        # ms per epoch tick (100 × 10ms = 1s max)
default_queue_capacity = 1024

[engine.fuel]
transform = 10_000_000
filter = 500_000
router = 500_000

# Per-node override
[nodes.heavy-transform]
type = "transform"
plugin = "plugins/heavy.wasm"
fuel = 50_000_000
```

---

## Decision 6: Edge Definition — Simplified

**Decision:** Edges use `from`/`to` as node ID strings. Optional `port` field (only needed on edges FROM a router — specifies which router output port). `to_port` removed (no Joiner). Queue config uses `capacity` and `overflow` directly on the edge.

**Rationale:** Only Router has named output ports. `port` is clearer than `from_port` since it's always the source port. Flat `capacity`/`overflow` fields are more readable than nested `[edges.queue]`.

**TOML:**
```toml
# Simple edge (most common)
[[edges]]
from = "source"
to = "filter"

# Edge from router (needs port)
[[edges]]
from = "router"
to = "transform-alerts"
port = "alert"

# Edge with queue config
[[edges]]
from = "filter"
to = "transform"
capacity = 2048
overflow = "drop"
```

**Rust:**
```rust
#[derive(Debug, Clone, Deserialize)]
pub struct EdgeDef {
    pub from: String,
    pub to: String,
    #[serde(default)]
    pub port: Option<String>,
    #[serde(default)]
    pub capacity: Option<usize>,
    #[serde(default)]
    pub overflow: Option<OverflowPolicy>,
}
```

---

## Decision 7: Remove DagConfig — Single Config Type

**Decision:** `DagConfig` struct deleted. `Config` is the only config type. `Config::dag_config()` and `from_dag_config()` removed. All builders take `&Config` directly.

**Rationale:** `DagConfig` is a historical artifact — a subset of `Config` that duplicates fields. Tests use `Config` with `Default` for sections they don't need.

---

## Decision 8: Top-Level Structure

**Decision:** Seven optional sections plus nodes and edges:

```toml
[pipeline]           # name, description
[engine]             # fuel, epoch, default_queue_capacity
[error_policy]       # pipeline-wide defaults for Wasm nodes
[dead_letter]        # DLQ sink config
[registry]           # OCI cache settings
[api]                # HTTP API server (optional, defaults enabled)
[metrics]            # Prometheus metrics (optional, defaults enabled)

[nodes.NAME]         # node definitions (map, keyed by ID)
[[edges]]            # edge connections (array)
```

All sections optional with sane defaults. Env var overrides supported via `WAFER_` prefix (e.g., `WAFER_API_BIND=0.0.0.0:8080`).

**Rationale:** Single-file, single-runtime model. A pipeline.toml fully defines one WAFER process. Deployment-specific values (bind addresses) overridable via env vars without modifying the file.

---

## Decision 9: Capabilities — Nested Table

**Decision:** Per-node capabilities live under `[nodes.X.capabilities]` as a nested table. Current: 3 booleans. Designed to grow with fine-grained grants post-thesis.

**Rationale:** Flat fields don't scale when security becomes more granular. A nested table accommodates future additions (filesystem paths, network egress, host import allowlists) without schema changes to the node level.

**TOML:**
```toml
[nodes.inference-transform.capabilities]
inherit_stdio = true
allow_inference = true
# inherit_env defaults to false

# Future (post-thesis):
# filesystem_read = ["/data/models/*"]
# network_egress = ["api.example.com:443"]
```

**Note:** `pipeline:host/logging` is always available (Session 1 F9) — not a configurable capability.

---

## Decision 10: Validation Strategy — Two-Phase with Accumulated Errors

**Decision:** Phase 1: Serde deserializes into strongly-typed structs (catches type/structure errors). Phase 2: `validate()` runs semantic checks and accumulates ALL errors before reporting.

**Rationale:** Best developer UX — shows every problem at once, not one at a time. Torvyn validates this pattern. Serde catches structure for free; semantic checks handle cross-field constraints.

**Semantic validation checks (Phase 2):**
- No cycles in DAG
- No orphan nodes (every node referenced by at least one edge)
- Router edges must specify `port`
- Only Transform/Sink may have multiple inbound edges
- Source nodes: zero inbound edges
- Sink nodes: zero outbound edges
- DLQ must be configured if any edge uses `overflow = "dead-letter"`
- `stdin`/`stdout` singleton constraint
- All `from`/`to` reference existing node IDs

---

## Decision 11: Aggressive Defaults for Minimal Config

**Decision:** Every field except `type` and type-specific required fields has a sensible default.

| Field | Default |
|-------|---------|
| `capacity` (edge) | 1024 |
| `overflow` (edge) | `"slow"` |
| Error policy | See Decision 4 defaults |
| Fuel | Per-type from `[engine.fuel]` |
| Capabilities | All false (sandbox) |
| `pipeline.name` | Filename stem |
| All `[engine]` fields | Session 2 defaults |
| `[dead_letter]` | None (disabled) |
| `[api]` | enabled, bind 127.0.0.1:9090 |
| `[metrics]` | enabled, path /metrics |

**Minimal viable config: 7 lines.**

---

## Decision 12: Source/Sink Config — Kind Enum + Flat Fields + Sub-Tables

**Decision:** `kind` is a required enum field on Source/Sink nodes. Core connection fields are flat on the node. Grouped concerns (TLS, auth) use sub-tables when needed.

**TOML:**
```toml
[nodes.mqtt-source]
type = "source"
kind = "mqtt"
broker = "broker.example.com"
port = 8883
topic = "sensors/+"
qos = 1
client_id = "wafer-prod"

[nodes.mqtt-source.tls]
ca = "/etc/ssl/ca.pem"
cert = "/etc/ssl/client.pem"
key = "/etc/ssl/client-key.pem"

[nodes.mqtt-source.auth]
username = "wafer"
password = "secret"
```

**Source kinds:** `mqtt`, `file`, `stdin`, `http`  
**Sink kinds:** `mqtt`, `file`, `stdout`, `http`

**Rust:**
```rust
#[derive(Debug, Clone, Deserialize)]
#[serde(tag = "kind", rename_all = "kebab-case")]
pub enum SourceDef {
    Mqtt(MqttSourceConfig),
    File(FileSourceConfig),
    Stdin(StdinSourceConfig),
    Http(HttpSourceConfig),
}

#[derive(Debug, Clone, Deserialize)]
pub struct MqttSourceConfig {
    pub broker: String,
    #[serde(default = "default_mqtt_port")]
    pub port: u16,
    pub topic: String,
    #[serde(default = "default_qos")]
    pub qos: u8,
    #[serde(default)]
    pub client_id: Option<String>,
    #[serde(default)]
    pub tls: Option<TlsConfig>,
    #[serde(default)]
    pub auth: Option<AuthConfig>,
}
```

---

## Decision 13: Dead Letter Queue — Reuses Sink Kind Pattern

**Decision:** `[dead_letter]` uses the same `kind` + flat fields as sink nodes. Adds `queue_capacity` as the only DLQ-specific field.

**TOML:**
```toml
[dead_letter]
kind = "file"
path = "/var/log/wafer-dlq.jsonl"
queue_capacity = 5000

# Or MQTT DLQ:
[dead_letter]
kind = "mqtt"
broker = "localhost"
port = 1883
topic = "dead-letter/messages"
queue_capacity = 10000
```

**Rationale:** Same construction logic as sinks. No special DLQ factory needed.

---

## Decision 14: Plugin Config — TOML Table Serialized to JSON

**Decision:** Per-node plugin configuration is a TOML table under `[nodes.X.config]`. At load time, the loader serializes it to a JSON string for passing to the WIT `init(node-config { id, config })` contract.

**Rationale:** Best DX — users write native TOML. The serialization to JSON is a one-line operation. Matches the WIT contract (init takes a string).

**TOML:**
```toml
[nodes.my-filter]
type = "filter"
plugin = "plugins/threshold-filter.wasm"

[nodes.my-filter.config]
threshold = 42
mode = "fast"
tags = ["alert", "high-priority"]
```

**At load time:**
```rust
let config_json = node_def.config
    .as_ref()
    .map(|table| serde_json::to_string(table))
    .transpose()?
    .unwrap_or_default();
```

---

## Complete Examples

### Simple Pipeline (Source → Filter → Transform → Sink)

```toml
[pipeline]
name = "iot-temperature"

[nodes.sensor-input]
type = "source"
kind = "mqtt"
broker = "localhost"
port = 1883
topic = "sensors/temperature"

[nodes.threshold-filter]
type = "filter"
plugin = "plugins/threshold-filter.wasm"

[nodes.threshold-filter.config]
min = 0
max = 100

[nodes.normalize]
type = "transform"
plugin = "plugins/normalize.wasm"

[nodes.output]
type = "sink"
kind = "stdout"

[[edges]]
from = "sensor-input"
to = "threshold-filter"

[[edges]]
from = "threshold-filter"
to = "normalize"

[[edges]]
from = "normalize"
to = "output"
```

### Complex Pipeline (Source → Filter → Router → [T₁, T₂] → Merge → Sink)

```toml
[pipeline]
name = "content-routing"
description = "Route messages by type, transform each branch, merge to output"

[engine.fuel]
transform = 10_000_000
filter = 500_000
router = 500_000

[error_policy]
bad_input = "dlq"
dependency_failed = { retries = 3, backoff_ms = 200, exhausted = "dlq" }
processing_failed = { retries = 2, backoff_ms = 100, exhausted = "dlq" }
timed_out = "skip"

[dead_letter]
kind = "file"
path = "/var/log/wafer-dlq.jsonl"

[nodes.input]
type = "source"
kind = "mqtt"
broker = "localhost"
port = 1883
topic = "events/raw"

[nodes.dedup-filter]
type = "filter"
plugin = "plugins/dedup-filter.wasm"

[nodes.router]
type = "router"
plugin = "plugins/content-router.wasm"

[nodes.transform-alerts]
type = "transform"
plugin = "ghcr.io/org/alert-enricher:1.2.0"
fuel = 20_000_000

[nodes.transform-alerts.error_policy]
dependency_failed = { retries = 10, backoff_ms = 500, exhausted = "dlq" }

[nodes.transform-logs]
type = "transform"
plugin = "plugins/log-formatter.wasm"

[nodes.output]
type = "sink"
kind = "mqtt"
broker = "localhost"
port = 1883
topic = "events/processed"

[[edges]]
from = "input"
to = "dedup-filter"

[[edges]]
from = "dedup-filter"
to = "router"

[[edges]]
from = "router"
to = "transform-alerts"
port = "alert"

[[edges]]
from = "router"
to = "transform-logs"
port = "log"

[[edges]]
from = "transform-alerts"
to = "output"

[[edges]]
from = "transform-logs"
to = "output"
```

### Minimal Viable Config (7 lines)

```toml
[nodes.in]
type = "source"
kind = "stdin"

[nodes.out]
type = "sink"
kind = "stdout"

[[edges]]
from = "in"
to = "out"
```

---

## What This Removes (vs Current Schema)

| Removed | Replacement |
|---------|-------------|
| `DagConfig` struct | `Config` only |
| `NodeConfig` internal struct | Typed `WasmNodeDef` |
| `node_type: NodeType` field | Serde `#[serde(tag = "type")]` |
| `source_type: Option<String>` | `kind: SourceKind` enum |
| `sink_type: Option<String>` | `kind: SinkKind` enum |
| `VALID_SOURCE_TYPES` / `VALID_SINK_TYPES` | Serde enum validation |
| `NodeConfig::validate()` | Auto-detect in `plugin_source()` |
| `to_port` on edges | Removed (no Joiner) |
| `from_port` on edges | Renamed to `port` |
| `NodeType::Joiner` variant | Deleted (merge is implicit) |
| `config: toml::Value` (untyped) | Typed per-variant structs |
| `Config::dag_config()` | Deleted |
| `from_dag_config()` builder | Takes `&Config` directly |
| `swappable` field | Deleted (all Wasm nodes swappable) |

---

## What This Adds

| Added | Purpose |
|-------|---------|
| `NodeType::Filter` variant | Session 1 separate filter interface |
| `[error_policy]` section | Session 2 D4 error policy engine |
| `[engine.fuel]` per-type defaults | Session 3 D10 fuel differentiation |
| `[nodes.X.capabilities]` table | Sandbox configuration (extensible) |
| `[nodes.X.config]` TOML table | Plugin init config (serialized to JSON) |
| `[nodes.X.error_policy]` override | Per-node error policy customization |
| `fuel` field on Wasm nodes | Per-node fuel override |
| Accumulated validation errors | Better developer UX |
| Env var override support | Deployment flexibility |

---

## Sources Consulted

| Source | What it informed |
|--------|-----------------|
| Torvyn `torvyn-config` crate (pipeline.rs, validate.rs) | Map-keyed nodes, edge endpoint structure, two-phase validation, accumulated errors |
| Torvyn examples (4 Torvyn.toml files) | Flow definition patterns, component reference format |
| Azure IoT dataflow-graphs YAML | Operation types, connection syntax, module references |
| Tremor Troy DSL | Connector/pipeline declarations, wiring syntax |
| tcc-doc findings §1.2-1.4 (Torvyn) | Config patterns, interface-based dispatch |
| tcc-doc findings §2.8 (Flow-Like) | Protobuf Board config, versioned caching |
| tcc-doc findings §3.6 (Wick) | Flow expression DSL |
| tcc-doc findings §7 (Tremor) | Contraflow, per-pipeline config |
| tcc-doc findings §9 (eKuiper) | SQL rules, REST config |
| Serde documentation (enum representations) | Internally-tagged enum for TOML dispatch |
| Web research: Rust serde validation patterns | Two-phase (parse + validate) vs fail-fast |
| Web research: TOML pipeline config (Edge Xpert, Azure IoT Edge) | IoT edge configuration conventions |
| Web research: OCI reference format (distribution-spec) | URI format for plugin references |
| Current WAFER schema.rs, loader.rs, diff.rs, builder.rs | Existing patterns to preserve/remove |
| 6 WAFER example TOMLs | Current usage patterns, edge cases |
| Session 1-3 decision docs | Constraints and requirements |
