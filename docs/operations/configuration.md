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

Both fields are optional; `name` is used in runtime logging. Node API responses do not include pipeline metadata.

## Tune the engine (`[engine]`)

The `[engine]` section governs wasmtime behaviour and default queue
sizing.

Runtime fuel limits and `epoch_deadline` default to `None`. Omit them for unlimited execution, or set positive values explicitly:

```toml
[engine]
epoch_deadline         = 100      # optional epoch ticks per Wasm call
epoch_tick_ms          = 10       # default wall-clock ms per epoch tick
default_queue_capacity = 1024     # fallback destination capacity

[engine.fuel]
transform = 10_000_000
filter    =    500_000
router    =    500_000
```

With both fields present, `epoch_deadline * epoch_tick_ms` is the nominal wall-clock cap per Wasm call. A positive `[nodes.NAME].fuel` value overrides the category value for one node. Zero is invalid. The values above are the protected final-evaluation policy, not runtime defaults.

Linear-memory limits default to 64 MiB for Transforms and 16 MiB for Filters and Routers. Change them per category, or for one node with `memory_limit` (bytes):

```toml
[engine.memory]
transform = 67_108_864
filter    = 16_777_216
router    = 16_777_216

[nodes.parse]
type         = "transform"
plugin       = "./plugins/json-parse/target/wasm32-wasip2/release/wafer_json_parse.wasm"
memory_limit = 33_554_432            # overrides [engine.memory].transform
```

After a Transform hot-swap, the previous component is kept for a rollback window that closes after `canary_success_count` successful calls or `canary_window_ms` milliseconds, whichever comes first. A trap inside the window (including a fuel or epoch budget trap), or an `unrecoverable` error, restores the previous component once. These are the defaults:

```toml
[engine.hot_swap]
canary_success_count = 32
canary_window_ms     = 10_000
```

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

A present `[nodes.NAME.error_policy]` table replaces the pipeline table for that node. Fields omitted from the node table use the built-in defaults; they do not inherit custom pipeline values.

The first retry waits exactly `backoff_ms`; subsequent delays double to the 30-second cap. The runtime selects the earliest due buffered retry. Exhaustion performs its configured `skip`, `dlq`, or `teardown` action, and DLQ-full/DLQ-closed outcomes remain distinct.

`unrecoverable` is intentionally not configurable — those errors always trigger node teardown and re-instantiation.

## Route the dead-letter queue (`[dead_letter]`)

Choose one variant.

```toml
# Ship DLQ to an MQTT broker
[dead_letter]
kind          = "mqtt"
broker        = "localhost"
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

A pipeline with a Transform, Filter or Router needs this section unless every
`dlq` action in its error policy is replaced by `skip` or `teardown`. A relative
file path is resolved against the working directory, or against
`WAFER_BENCH_OUTPUT_DIR` when that variable is set.

## Define nodes (`[nodes.NAME]`)

Nodes are a **map** keyed by id; the `type` field discriminates the variant. Transform, Filter, and Router use one `plugin` field. A string selects a local path or OCI reference; `{ kind = "native", function = "..." }` selects a built-in evaluation baseline. Only loaded Wasm implementations are replacement-eligible.

```toml
[nodes.mqtt-in]
type   = "source"
kind   = "mqtt"
broker = "localhost"
topic  = "sensors/#"
qos    = 1

[nodes.parse]
type           = "transform"
plugin         = "./plugins/json-parse/target/wasm32-wasip2/release/wafer_json_parse.wasm"
plugin_version = "v1"                # opaque value passed to lifecycle.init
fuel           = 5_000_000            # override [engine.fuel.transform] for this node

[nodes.parse.capabilities]
inherit_stdio   = false
inherit_env     = false
allow_inference = false
outbound_http   = []

[nodes.parse.config]                 # free-form; serialised to JSON, passed to lifecycle.init
strict = true

[nodes.parse.error_policy]
bad_input = "skip"                   # replaces the pipeline table for this node

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
broker = "localhost"
topic  = "alerts"

[nodes.log-sink]
type = "sink"
kind = "stdout"
```

### Grant bounded outbound HTTP

A Wasm Transform, Filter, or Router can import P2 `wasi:http`. The host links the interface for every processing component, but requests are denied unless their normalized scheme, host, and effective port match an explicit grant:

```toml
[nodes.enrich.capabilities]
outbound_http = [
  { scheme = "https", host = "api.example.com" },
  { scheme = "http", host = "127.0.0.1", port = 8080 },
]
```

Omission and an empty list both deny all destinations. Use only `http` or `https`; omitted ports become 80 or 443. Wildcards, CIDR blocks, paths, queries, user information, duplicate normalized destinations, and port zero fail validation. DNS grants connect only to globally routable addresses from one resolution snapshot. Grant an exact IP literal when intentional loopback or private-network access is required. Redirects are returned to the guest rather than followed by the host, and `CONNECT` is always denied.

The grant is immutable until pipeline restart. Recovery, reconfigure, hot-swap, and rollback retain the original set; guest configuration cannot add authority. Native sources and sinks remain the transport boundary when credentials, retries, long-lived connections, backpressure, or delivery semantics matter. See the [configuration reference](../interfaces/config-schema.md#outbound-wasihttp-capability) and [ADR-0016](../adr/0016-outbound-wasi-http-capability.md).

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

Source, Transform, and Filter broadcast to every downstream edge. Router sends only to edges whose `port` was returned by the guest.

Fan-in is implicit — if two edges terminate at the same node, the host wires multiple senders onto that node's single `mpsc` receiver. `capacity` is still written on edges, but the physical queue is receiver-keyed: its capacity is the maximum explicit incoming capacity, or `engine.default_queue_capacity` only when no incoming edge declares one. Producer ordering and fairness are not guaranteed.

All queue, retry-buffer, and DLQ capacities must be greater than zero; validation rejects zero before any channel is created. `slow` waits for a permit, `drop` discards only when the destination is full, and `dead-letter` makes a non-blocking attempt to the configured file or MQTT DLQ. Destination closed, DLQ full, and DLQ closed are separate outcomes.

## Expose the control plane and metrics (`[api]` / `[metrics]`)

```toml
[api]
enabled = true
bind    = "127.0.0.1:9090"

[metrics]
enabled = true
path    = "/metrics"        # used only by a separate --metrics-bind listener
```

Metrics can be served on the same axum port as the API (default) or
on a dedicated one; see [`observability.md`](observability.md).

## Point at an OCI registry cache (`[registry]`)

Optional; only relevant when a `plugin` field is an OCI reference.

```toml
[registry]
cache_dir = "/var/cache/wafer/oci"
```

See [`registry.md`](registry.md) for publishing, pulling, and caching.

## Validate before you run

Parsing rejects unknown keys in every section, node and edge (only a node's `config` table is free-form), so a misspelled key fails startup instead of falling back to a default. Every config is then validated by `wafer_config::validate` before plugin loading or pipeline construction. Failures include a missing referenced node id, graph cycle, router edge without `port`, `port` on an edge that does not leave a router, a duplicated edge, a transform, filter or router without both an inbound and an outbound edge, zero capacity or `epoch_tick_ms`, an inference grant on an ineligible role, or an invalid outbound HTTP destination. `allow_inference = true` is accepted only for a Wasm Transform; native Transforms, Filters, and Routers fail with `allow_inference=true is supported only for Wasm Transform nodes`. Non-empty `outbound_http` lists are accepted only for Wasm processing nodes.

The runtime has no standalone `--check` flag. Repository examples and evaluation configs are validated by the `wafer-config` test suites; starting `wafer-runtime --config <path>` also validates before launch.

## Full example — telemetry gateway

The snippets above combine into a working config; look at
`examples/dag-mqtt.toml` for a version with realistic defaults and
comments.
