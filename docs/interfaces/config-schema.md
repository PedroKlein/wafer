# Config Schema Reference

Reference for the TOML pipeline configuration consumed by `wafer-runtime --config <path>`. The domain types live in `crates/wafer-types/src/config/`; the loader / validator / DAG builder live in `crates/wafer-config/`, and the runtime binary imports them directly (gap **A1**: Closed 2026-07-19).

Every top-level section is optional. Defaults describe runtime behavior, not the final evaluation policy. Final WAFER configs set metering explicitly and are checked against `eval/canonical-matrix.json`.

## Top-level sections

| Section | Struct | Purpose |
|---------|--------|---------|
| `[pipeline]` | `PipelineConfig` | Human-readable name / description. |
| `[engine]` | `EngineConfig` | Wasmtime knobs (epoch, fuel, default queue capacity). |
| `[error_policy]` | `ErrorPolicyConfig` | Pipeline-wide error-handling defaults. |
| `[dead_letter]` | `DeadLetterConfig` | Where DLQ envelopes go. |
| `[registry]` | `RegistryConfig` | OCI cache directory. |
| `[api]` | `ApiConfig` | HTTP control plane. |
| `[metrics]` | `MetricsConfig` | Prometheus exposition. |
| `[nodes.NAME]` | `NodeDef` | One entry per node (map keyed by id). |
| `[[edges]]` | `EdgeDef` | Array of edges. |

## `[pipeline]`

```toml
[pipeline]
name        = "telemetry-gateway"      # optional
description = "MQTT -> filter -> alert"  # optional
```

## `[engine]`

| Field | Type | Runtime default | Notes |
|-------|------|-----------------|-------|
| `epoch_deadline` | `Option<NonZeroU64>` | `None` | Epoch ticks per Wasm call. Omission disables epoch interruption. |
| `epoch_tick_ms` | `u64` | `10` | Wall-clock milliseconds per epoch tick. A ticker alone does not impose a deadline. |
| `default_queue_capacity` | `usize` | `1024` | Fallback capacity for edges without their own. |
| `fuel.transform` | `Option<NonZeroU64>` | `None` | Fuel per Transform call. Omission disables fuel for this category. |
| `fuel.filter` | `Option<NonZeroU64>` | `None` | Fuel per Filter call. |
| `fuel.router` | `Option<NonZeroU64>` | `None` | Fuel per Router call. |
| `memory.transform` | `usize` | `67_108_864` | Transform linear-memory limit, 64 MiB. |
| `memory.filter` | `usize` | `16_777_216` | Filter linear-memory limit, 16 MiB. |
| `memory.router` | `usize` | `16_777_216` | Router linear-memory limit, 16 MiB. |

Fuel and epoch values must be positive when present. Zero is rejected; omission produces `None`. `[nodes.NAME.fuel]` overrides a configured pipeline fuel value for one Wasm node.

Ordinary final WAFER evaluation configs set Transform fuel to `10_000_000`, Filter and Router fuel to `500_000`, `epoch_deadline` to `100`, and `epoch_tick_ms` to `10`. These are evaluation values, not runtime defaults. E-Perf-7 disables a mechanism by omitting its field:

| Mode | `fuel.transform` | `epoch_deadline` |
|---|---|---|
| `neither` | omitted (`None`) | omitted (`None`) |
| `fuel-only` | `10_000_000` | omitted (`None`) |
| `epoch-only` | omitted (`None`) | `100` |
| `both` | `10_000_000` | `100` |

## `[error_policy]`

Pipeline-wide default. `[nodes.NAME.error_policy]` overrides the pipeline-level
table for a specific node; the runner reads the resolved policy at pipeline
start via `resolve_error_policy` in `crates/wafer-core/src/orchestrator/builder.rs`.
The default `retry_buffer_capacity` is 1000.

| Field | Type | Default | Notes |
|-------|------|---------|-------|
| `bad_input` | `SimpleAction` | `"dlq"` | Action for `bad-input` errors. |
| `dependency_failed` | `RetryConfig` | `{retries=3, backoff_ms=100, exhausted="dlq"}` | Retry with backoff, then fall back. |
| `processing_failed` | `RetryConfig` | `{retries=2, backoff_ms=100, exhausted="dlq"}` | Same shape. |
| `timed_out` | `SimpleAction` | `"skip"` | Fuel / epoch interrupt. |
| `retry_buffer_capacity` | `usize` | `1000` | Bounded VecDeque; overflow -> DLQ with `RetryBufferFull`. |

`SimpleAction` values (`#[serde(rename_all = "kebab-case")]`): `skip | dlq | teardown`.

`RetryConfig` fields: `retries: u32`, `backoff_ms: u64`, `exhausted: SimpleAction`.

`unrecoverable` errors are not configurable: they always trigger a node teardown and re-instantiation from the cached `InstancePre`.

## `[dead_letter]`

Tagged variant on `kind`:

```toml
# MQTT DLQ
[dead_letter]
kind          = "mqtt"
broker        = "mqtt://localhost"
port          = 1883
topic         = "wafer/dlq"
queue_capacity = 10000        # default 10 000
# optional: tls = {...}, auth = {...}
```

```toml
# File DLQ
[dead_letter]
kind          = "file"
path          = "/var/log/wafer/dlq.jsonl"
queue_capacity = 5000         # default 5000
```

## `[registry]`

```toml
[registry]
cache_dir = "/var/cache/wafer/oci"   # optional; defaults to XDG cache
```

## `[api]`

| Field | Type | Default |
|-------|------|---------|
| `enabled` | `bool` | `true` |
| `bind` | `string` | `"127.0.0.1:9090"` |

## `[metrics]`

| Field | Type | Default |
|-------|------|---------|
| `enabled` | `bool` | `true` |
| `path` | `string` | `"/metrics"` (informational; the wired path is `/metrics`) |

## `[nodes.NAME]`

Nodes are a **map** keyed by node id. The `type` field discriminates the variant.

```toml
[nodes.src]
type = "source"       # source | sink | transform | filter | router
# ...variant-specific fields...
```

### Wasm nodes (`transform`, `filter`, `router`)

Struct `WasmNodeDef`:

| Field | Type | Notes |
|-------|------|-------|
| `plugin` | `string` (required) | Local filesystem path OR OCI reference (`ghcr.io/user/foo:tag`). Auto-detected by the loader; **single field: no `plugin_path` / `plugin_ref` split**. |
| `fuel` | `Option<u64>` | Overrides the pipeline default for this node. |
| `capabilities` | `Capabilities` | `{inherit_stdio, inherit_env, allow_inference}`, all default `false`. |
| `config` | `Option<toml::Value>` | Free-form plugin config; serialised to JSON and passed to `lifecycle.init` as `node-config.config`. |
| `error_policy` | `Option<ErrorPolicyConfig>` | Per-node override that replaces the pipeline-level table for this node when present. |

Example:

```toml
[nodes.parse]
type   = "transform"
plugin = "./plugins/json-parse/target/wasm32-wasip2/release/wafer_json_parse.wasm"
fuel   = 5_000_000

[nodes.parse.capabilities]
inherit_stdio = false
inherit_env   = false
allow_inference = false

[nodes.parse.config]
strict = true

[nodes.parse.error_policy]
bad_input = "skip"          # override pipeline default of "dlq"
```

### Source and Sink

Both are tagged by `kind` (`#[serde(rename_all = "kebab-case")]`).

Sources (`SourceDef`): `mqtt | file | stdin | http | bench-source`.
Sinks (`SinkDef`): `mqtt | file | stdout | http | bench-sink`.

| Kind | Config fields |
|------|---------------|
| `mqtt` (source) | `broker`, `port` (default 1883), `topic`, `qos: u8` (default 0), `client_id`, optional `tls`, optional `auth`. |
| `mqtt` (sink) | Same as source + `retain: bool` (default `false`). |
| `file` (source) | `path`. |
| `file` (sink) | `path`, `append: bool` (default `true`). |
| `stdin` (source) | (no fields) |
| `stdout` (sink) | (no fields) |
| `http` (source) | `bind` (default `127.0.0.1:8081`), `path` (default `/webhook`). |
| `http` (sink) | `url`, `method` (default `POST`). |
| `bench-source` | `rate`, `total_messages`, `warmup_messages` (default `0`), `payload_size` (default `128`), optional `burst`. |
| `bench-sink` | `warmup_secs` (default `30`), `track_sequences` (default `true`), `track_hotswap` (default `false`), optional `output_dir`. |

A `bench-source.burst` table contains `rate`, `start_secs`, and `end_secs`. Final E-Swap-4 uses base rate 1,000 msg/s, burst rate 2,000 msg/s, start 55 seconds, and end 65 seconds after measurement begins.

`TlsConfig`: optional `ca`, `cert`, `key` file paths.
`AuthConfig`: `username`, `password`.

Example:

```toml
[nodes.mqtt-in]
type   = "source"
kind   = "mqtt"
broker = "mqtt://localhost"
topic  = "sensors/#"
qos    = 1
```

## `[[edges]]`

Struct `EdgeDef`:

| Field | Type | Notes |
|-------|------|-------|
| `from` | `string` (required) | Node id. |
| `to` | `string` (required) | Node id. |
| `port` | `Option<string>` | Router output port name. Only required when `from` is a router. **Single `port` field: no `from_port` / `to_port` split.** |
| `capacity` | `Option<usize>` | Per-edge queue capacity; falls back to `engine.default_queue_capacity`. |
| `overflow` | `Option<OverflowPolicy>` | `slow` (default; backpressure) \| `drop` \| `dead-letter`. |

Example:

```toml
[[edges]]
from = "src"
to   = "parse"

[[edges]]
from     = "router"
to       = "alert"
port     = "alert"        # router output port
capacity = 4096
overflow = "dead-letter"
```

## Minimal example

```toml
[nodes.in]
type = "source"
kind = "stdin"

[nodes.out]
type = "sink"
kind = "stdout"

[[edges]]
from = "in"
to   = "out"
```

Every other field has a sensible default; the runtime uses this snippet as its smoke test.
