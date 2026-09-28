# Config Schema Reference

Reference for the TOML pipeline configuration consumed by `wafer-runtime --config <path>`. The domain types live in `crates/wafer-types/src/config/`; the loader / validator / DAG builder live in `crates/wafer-config/`, and the runtime binary imports them directly (gap **A1**: Closed 2026-07-19).

Every top-level section is optional. Defaults describe runtime behavior, not the final evaluation policy. Final WAFER configs set metering explicitly and are checked against `eval/canonical-matrix.json`.

Unknown keys are rejected at load time in every section, node and edge, so a misspelled key fails startup instead of silently falling back to its default. The only free-form table is a processing node's `config`, which is passed to the plugin unchanged.

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
| `epoch_tick_ms` | `u64` | `10` | Wall-clock milliseconds per epoch tick. Must be greater than zero. A ticker alone does not impose a deadline. |
| `default_queue_capacity` | `usize` | `1024` | Fallback capacity when no incoming edge for a destination declares one. |
| `fuel.transform` | `Option<NonZeroU64>` | `None` | Fuel per Transform call. Omission leaves the category unlimited (see below). |
| `fuel.filter` | `Option<NonZeroU64>` | `None` | Fuel per Filter call. |
| `fuel.router` | `Option<NonZeroU64>` | `None` | Fuel per Router call. |
| `memory.transform` | `usize` | `67_108_864` | Transform linear-memory limit, 64 MiB. |
| `memory.filter` | `usize` | `16_777_216` | Filter linear-memory limit, 16 MiB. |
| `memory.router` | `usize` | `16_777_216` | Router linear-memory limit, 16 MiB. |
| `hot_swap.canary_success_count` | `u32` | `32` | Successful Transform calls after a swap that close the rollback window. |
| `hot_swap.canary_window_ms` | `u64` | `10_000` | Wall-clock milliseconds after a swap that close the rollback window, whichever comes first. |

Fuel and epoch values must be positive when present. Zero is rejected; omission produces `None`. Queue capacities must be greater than zero: the validator rejects zero for `default_queue_capacity`, `[[edges]].capacity`, retry buffers, and DLQ queues before channel construction. `[nodes.NAME.fuel]` overrides a configured pipeline fuel value for one Wasm node.

Fuel metering is engine-wide. It is off only when no `fuel.*` value and no node `fuel` is set. Once any is set, every Wasm node is refilled before each call with its own `fuel`, else its category's value, else `u64::MAX`, so an omitted category stays unlimited in practice while still paying the metering cost.

If a replaced Transform traps (including a fuel or epoch budget trap) or returns `unrecoverable` while its rollback window is open, the runtime restores the previous component once and closes the window; a later trap is handled like any other failure of the restored component.

Ordinary final WAFER evaluation configs set Transform fuel to `10_000_000`, Filter and Router fuel to `500_000`, `epoch_deadline` to `100`, and `epoch_tick_ms` to `10`. These are evaluation values, not runtime defaults. E-Perf-7 disables a mechanism by omitting its field (its pipelines set no other fuel value):

| Mode | `fuel.transform` | `epoch_deadline` |
|---|---|---|
| `neither` | omitted (`None`) | omitted (`None`) |
| `fuel-only` | `10_000_000` | omitted (`None`) |
| `epoch-only` | omitted (`None`) | `100` |
| `both` | `10_000_000` | `100` |

## `[error_policy]`

Pipeline-wide default. A present `[nodes.NAME.error_policy]` table replaces the pipeline table for that node; omitted fields within the node table use `ErrorPolicyConfig` defaults rather than inheriting custom pipeline values. The runner resolves this choice at pipeline start in `crates/wafer-core/src/orchestrator/builder.rs`. The default `retry_buffer_capacity` is 1000.

| Field | Type | Default | Notes |
|-------|------|---------|-------|
| `bad_input` | `SimpleAction` | `"dlq"` | Action for `bad-input` errors. |
| `dependency_failed` | `RetryConfig` | `{retries=3, backoff_ms=100, exhausted="dlq"}` | Retry with backoff, then fall back. |
| `processing_failed` | `RetryConfig` | `{retries=2, backoff_ms=100, exhausted="dlq"}` | Same shape. |
| `timed_out` | `SimpleAction` | `"skip"` | Fuel / epoch interrupt. |
| `retry_buffer_capacity` | `usize` | `1000` | Bounded VecDeque; overflow -> DLQ with `RetryBufferFull`. |

`SimpleAction` values (`#[serde(rename_all = "kebab-case")]`): `skip | dlq | teardown`.

`RetryConfig` fields: `retries: u32`, `backoff_ms: u64`, `exhausted: SimpleAction`. The first retry waits exactly `backoff_ms`; later delays double up to 30 seconds. The bounded buffer selects the earliest due entry, so an earlier-deadline retry cannot be stranded behind a later one. Exhaustion honors the configured `skip`, `dlq`, or `teardown` action. DLQ-full and DLQ-closed outcomes are distinct and exhausted envelopes are not requeued.

`unrecoverable` errors and traps are not configurable: the message is written to the DLQ (reason `unrecoverable` or `trapped`) and the node is re-instantiated from the cached `InstancePre`.

## `[dead_letter]`

Tagged variant on `kind`:

```toml
# MQTT DLQ
[dead_letter]
kind          = "mqtt"
broker        = "localhost"
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

The configured sink is active: the runtime drains DLQ records to the MQTT topic or file, one JSON object per record. Each record carries `timestamp`, `source_node`, `error_category` (null for a trap or queue overflow), `error_message`, `retry_count`, `reason`, `original` (the envelope, payload base64-encoded), `trace_id` and `parent_id`. `reason.type` is one of `bad_input`, `timed_out`, `retries_exhausted`, `retry_buffer_full`, `hot_swap_drain`, `shutdown`, `queue_full`, `trapped` (with `kind`), `unrecoverable` or `recovery_failed`. Destination closed, DLQ full, and DLQ closed are separate counters rather than successful dead-letter delivery.

A relative file `path` is resolved against the working directory, or against `WAFER_BENCH_OUTPUT_DIR` when the runtime is started with it set, which is how the evaluation configs put `dlq.jsonl` into the run's result directory. The file sink writes each record as it arrives and keeps draining until every node has exited.

## `[registry]`

```toml
[registry]
cache_dir = "/var/cache/wafer/oci"   # optional; defaults to XDG cache
```

`cache_dir` is the only key. Other registry settings such as `cache_ttl_hours`
or `verify_cosign` are not config keys, so they fail the unknown-key check.

## `[api]`

| Field | Type | Default |
|-------|------|---------|
| `enabled` | `bool` | `true` |
| `bind` | `string` | `"127.0.0.1:9090"` (the `--api-bind <ADDR>` option overrides it) |

## `[metrics]`

| Field | Type | Default |
|-------|------|---------|
| `enabled` | `bool` | `true` |
| `path` | `string` | `"/metrics"` (used only by the separate listener that `--metrics-bind <ADDR>` starts; on the API listener the path is always `/metrics`) |

## `[nodes.NAME]`

Nodes are a **map** keyed by node id. The `type` field discriminates the variant.

```toml
[nodes.src]
type = "source"       # source | sink | transform | filter | router
# ...variant-specific fields...
```

### Processing nodes (`transform`, `filter`, `router`)

The configuration struct remains `WasmNodeDef`, but `plugin` can select a Wasm component or a closed native evaluation function:

| Field | Type | Notes |
|-------|------|-------|
| `plugin` | `string` or tagged inline table (required) | A string selects a local/OCI Wasm component. `{ kind = "wasm", path = "..." }` is the explicit equivalent. `{ kind = "native", function = "..." }` selects a built-in baseline. Only loaded Wasm implementations are replacement-eligible. |
| `fuel` | `Option<u64>` | Overrides the pipeline default for this node. Setting it turns fuel metering on for the engine. Zero is rejected. |
| `memory_limit` | `Option<usize>` | Linear-memory limit in bytes for this node, used at launch, recovery, reconfigure and hot-swap. Omission uses `[engine.memory]` for the node's category. |
| `capabilities` | `Capabilities` | `{inherit_stdio, inherit_env, allow_inference, outbound_http}`. Booleans default to `false`; `outbound_http` defaults to an empty deny-all list. `allow_inference=true` selects the inference linker and store only for a Wasm Transform. |
| `config` | `Option<toml::Value>` | Free-form plugin config; serialised to JSON and passed to `lifecycle.init` as `node-config.config`. |
| `error_policy` | `Option<ErrorPolicyConfig>` | Per-node table that replaces the pipeline-level table when present. |
| `plugin_version` | `Option<string>` | Opaque version passed as `node-config.plugin-version`; default is empty. |

`allow_inference = true` is valid only when the node is a Wasm Transform. It
fails semantic validation for a native Transform, Filter, or Router with:
`allow_inference=true is supported only for Wasm Transform nodes`. Source and
Sink variants have no processing-node capability table. When omitted or false,
the ordinary linker contains no wasi-nn imports, so an inference component
fails closed during preparation.

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

#### Outbound `wasi:http` capability

[ADR-0016](../adr/0016-outbound-wasi-http-capability.md) defines this optional P2-compatible capability. WAFER links wasi:http for Wasm processing nodes but grants no destination unless `outbound_http` contains an exact entry.

```toml
[nodes.enrich.capabilities]
outbound_http = [
  { scheme = "https", host = "api.example.com" },       # effective port 443
  { scheme = "http", host = "127.0.0.1", port = 8080 },
]
```

Omission and `outbound_http = []` both deny every outbound request. A destination contains only `scheme`, `host`, and optional `port`. Scheme is exactly `http` or `https`; omitted ports normalize to 80 or 443 respectively. Matching uses the normalized `(scheme, host, effective-port)` tuple. Wildcards, CIDR blocks, suffixes, paths, queries, fragments, user information, port ranges, and port zero are invalid.

DNS names use lowercase ASCII comparison without a trailing dot. DNS grants may connect only to globally routable unicast addresses from a single policy-controlled resolution snapshot. Intentional loopback or private-network access requires an exact IP-literal grant. Unspecified, multicast, broadcast, and link-local literals are never grantable. Redirects are returned to the guest and are not followed automatically.

The grant is immutable until pipeline restart. Recovery, reconfigure, hot-swap, and rollback reconstruct the exact original set; guest lifecycle JSON and hot-swap request bodies cannot add destinations. Denial is returned as `wasi:http` `http-request-denied`, without logging request bodies, query strings, cookies, authorization values, or arbitrary headers.

### Source and Sink

Both are tagged by `kind` (`#[serde(rename_all = "kebab-case")]`).

Sources (`SourceDef`): `mqtt | file | stdin | http | bench-source`.
Sinks (`SinkDef`): `mqtt | file | stdout | http | bench-sink`.

| Kind | Config fields |
|------|---------------|
| `mqtt` (source) | `broker`, `port` (default 1883), `topic`, `qos: u8` (default 0), optional `client_id` (default `wafer-<node id>`), optional `tls`, optional `auth`. `tls` and `auth` are accepted but not applied: the connection is always plain TCP without credentials. |
| `mqtt` (sink) | Same as source + `retain: bool` (default `false`). `retain` is accepted but not applied: every message is published with retain off. `tls` and `auth` are not applied either. |
| `file` (source) | `path`. |
| `file` (sink) | `path`, `append: bool` (default `true`). `append` is accepted but not applied: the sink truncates the file when it starts. |
| `stdin` (source) | (no fields) |
| `stdout` (sink) | (no fields) |
| `http` (source) | `bind` (default `127.0.0.1:8081`), `path` (default `/webhook`). |
| `http` (sink) | `url`, `method` (default `POST`). `method` is accepted but not applied: every request is a `POST`. |
| `bench-source` | `rate`, `total_messages`, `warmup_messages` (default `0`), `payload_size` (default `128`), optional `burst`. |
| `bench-sink` | `warmup_secs` (default `30`), `track_sequences` (default `true`), `track_hotswap` (default `false`), optional `output_dir`. |

A `bench-source.burst` table contains `rate`, `start_secs`, and `end_secs`. Final E-Swap-4 uses base rate 1,000 msg/s, burst rate 2,000 msg/s, start 55 seconds, and end 65 seconds after measurement begins.

`TlsConfig`: optional `ca`, `cert`, `key` file paths.
`AuthConfig`: `username`, `password`.

Only `[dead_letter] kind = "mqtt"` applies `tls` and `auth` to its broker connection.

Example:

```toml
[nodes.mqtt-in]
type   = "source"
kind   = "mqtt"
broker = "localhost"
topic  = "sensors/#"
qos    = 1
```

## `[[edges]]`

Struct `EdgeDef`:

| Field | Type | Notes |
|-------|------|-------|
| `from` | `string` (required) | Node id. |
| `to` | `string` (required) | Node id. |
| `port` | `Option<string>` | Router output port name. Required when `from` is a router and rejected otherwise. **Single `port` field: no `from_port` / `to_port` split.** |
| `capacity` | `Option<usize>` | Requested destination queue capacity. One physical receiver is created per destination; its capacity is the maximum explicit incoming capacity, or `engine.default_queue_capacity` when none is specified. |
| `overflow` | `Option<OverflowPolicy>` | Sender-side policy for this edge: `slow` (default; reserve/await), `drop` (non-blocking discard on full), or `dead-letter` (non-blocking DLQ attempt on full). |

Edge rules are listed under [Validation](#validation).

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

## Validation

After parsing, the loader runs every check below and reports all failures
together; the runtime exits with status 2 and starts nothing.

- Every edge's `from` and `to` name a configured node.
- Sources have no inbound edge; sinks have no outbound edge.
- A filter or router has exactly one inbound edge.
- Every edge from a router has a `port`, and no other edge has one.
- The same `from`, `to` and `port` is not listed twice, which would deliver every message twice.
- A transform, filter or router has at least one inbound and one outbound edge.
- Every node is connected to an edge, and the graph has no cycle.
- At most one `stdin` source and one `stdout` sink.
- An edge with `overflow = "dead-letter"` requires a `[dead_letter]` section, and so does any Transform, Filter or Router whose effective `bad_input`, `timed_out`, `dependency_failed.exhausted` or `processing_failed.exhausted` action is `dlq` (the per-node table when present, otherwise `[error_policy]`).
- `engine.default_queue_capacity`, `[[edges]].capacity`, both `retry_buffer_capacity` levels, and `dead_letter.queue_capacity` are greater than zero; so is `engine.epoch_tick_ms`.
- `outbound_http` appears only on Wasm nodes, and its destinations are valid and not duplicated after normalization.
- `allow_inference = true` appears only on a Wasm Transform.

Zero for `epoch_deadline` or any fuel value is rejected while parsing.
Because `bad_input` and both `exhausted` actions default to `dlq`, a pipeline
with a processing node needs `[dead_letter]` unless it sets those actions to
`skip` or `teardown`.

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

For fan-in, `capacity` remains edge-shaped configuration but the physical queue is receiver-keyed. If any incoming edge declares a capacity, the destination uses the maximum explicit incoming capacity; the default is used only when none declares one. Tokio does not promise fair ordering across producers.
