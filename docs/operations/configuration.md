# Configure a Pipeline

Every WAFER pipeline is declared in a single TOML file. This how-to
walks through each configuration section by task. For the full field
reference, see [`../interfaces/config-schema.md`](../interfaces/config-schema.md).

Sample configs live under `examples/`; the smallest working example is
`examples/dag-passthrough.toml`.

## Name your pipeline

```toml
[pipeline]
name        = "telemetry-gateway"
description = "MQTT sensor ingest → threshold filter → alert / log"
```

Both fields are optional and appear in log output plus the `/api/v1/nodes` response header.

## Tune the engine (`[engine]`)

The `[engine]` section governs wasmtime behaviour and default queue
sizing.

Runtime fuel limits and `epoch_deadline` default to `None`. Omit them for unlimited execution, or set positive values explicitly:

```toml
[engine]
epoch_deadline         = 100      # optional epoch ticks per Wasm call
epoch_tick_ms          = 10       # default wall-clock ms per epoch tick
default_queue_capacity = 1024     # default per-edge capacity

[engine.fuel]
transform = 10_000_000
filter    =    500_000
router    =    500_000
```

With both fields present, `epoch_deadline * epoch_tick_ms` is the nominal wall-clock cap per Wasm call. A positive `[nodes.NAME].fuel` value overrides the category value for one node. Zero is invalid. The values above are the protected final-evaluation policy, not runtime defaults.

## Configure the error policy (`[error_policy]`)

The five error categories from the WIT `process-error` variant map to
host actions here. The block below is the built-in default; every
field is optional.

```toml
[error_policy]
bad_input             = "dlq"                                # skip | dlq | teardown
timed_out             = "skip"
retry_buffer_capacity = 1000

[error_policy.dependency_failed]
retries    = 3
backoff_ms = 100
exhausted  = "dlq"

[error_policy.processing_failed]
retries    = 2
backoff_ms = 100
exhausted  = "dlq"
```

Per-node override in `[nodes.NAME.error_policy]` merges field-by-field
on top of these values.

`unrecoverable` is intentionally not configurable — those errors
always trigger node teardown and re-instantiation.

## Route the dead-letter queue (`[dead_letter]`)

Choose one variant.

```toml
# Ship DLQ to an MQTT broker
[dead_letter]
kind          = "mqtt"
broker        = "mqtt://localhost"
port          = 1883
topic         = "wafer/dlq"
queue_capacity = 10000
```

```toml
# ...or to a local file
[dead_letter]
kind          = "file"
path          = "/var/log/wafer/dlq.jsonl"
queue_capacity = 5000
```

## Define nodes (`[nodes.NAME]`)

Nodes are a **map** keyed by id; the `type` field discriminates the
variant. Wasm variants (`transform`, `filter`, `router`) always have a
single `plugin` field — local path or OCI reference, auto-detected.

```toml
[nodes.mqtt-in]
type   = "source"
kind   = "mqtt"
broker = "mqtt://localhost"
topic  = "sensors/#"
qos    = 1

[nodes.parse]
type   = "transform"
plugin = "./plugins/json-parse/target/wasm32-wasip2/release/wafer_json_parse.wasm"
fuel   = 5_000_000                   # override [engine.fuel.transform] for this node

[nodes.parse.capabilities]
inherit_stdio    = false
inherit_env      = false
allow_inference  = false

[nodes.parse.config]                 # free-form; serialised to JSON, passed to lifecycle.init
strict = true

[nodes.parse.error_policy]
bad_input = "skip"                   # override pipeline default of "dlq"

[nodes.threshold]
type   = "filter"
plugin = "./plugins/threshold-filter/target/wasm32-wasip2/release/wafer_threshold_filter.wasm"

[nodes.threshold.config]
field     = "temperature"
threshold = 40.0

[nodes.route]
type   = "router"
plugin = "./plugins/content-router/target/wasm32-wasip2/release/wafer_content_router.wasm"

[nodes.route.config]
field = "level"

[nodes.alert-sink]
type   = "sink"
kind   = "mqtt"
broker = "mqtt://localhost"
topic  = "alerts"

[nodes.log-sink]
type = "sink"
kind = "stdout"
```

## Wire edges (`[[edges]]`)

Edges are an array. Every edge has `from`, `to`, and optional
`capacity` / `overflow`. **A single `port` field** is used for router
outputs — no `from_port` / `to_port` split.

```toml
[[edges]]
from = "mqtt-in"
to   = "parse"

[[edges]]
from = "parse"
to   = "threshold"

[[edges]]
from = "threshold"
to   = "route"

# Router → sinks: `port` names one of the router's declared output-ports.
[[edges]]
from = "route"
to   = "alert-sink"
port = "alert"

[[edges]]
from     = "route"
to       = "log-sink"
port     = "log"
capacity = 4096
overflow = "dead-letter"
```

Fan-in is implicit — if two edges terminate at the same node, the host
wires multiple senders onto that node's single `mpsc` receiver.

## Expose the control plane and metrics (`[api]` / `[metrics]`)

```toml
[api]
enabled = true
bind    = "127.0.0.1:9090"

[metrics]
enabled = true
path    = "/metrics"        # informational; the wired path is /metrics
```

Metrics can be served on the same axum port as the API (default) or
on a dedicated one; see [`observability.md`](observability.md).

## Point at an OCI registry cache (`[registry]`)

Optional; only relevant when a `plugin` field is an OCI reference.

```toml
[registry]
cache_dir = "/var/cache/wafer/oci"
```

See [`registry.md`](registry.md) for publishing, pulling, and cosign
verification.

## Validate before you run

Every config is validated by `wafer_config::validate` at startup.
Failures include: missing referenced node id, cycle in the graph,
router edge without a declared `port`, unknown `type`, and empty
`plugin` field. Run the runtime with `--check` to validate without
starting the pipeline:

```bash
cargo run -p wafer-runtime -- --config my-pipeline.toml --check
```

## Full example — telemetry gateway

The snippets above combine into a working config; look at
`examples/dag-mqtt.toml` for a version with realistic defaults and
comments.
