# Configure a Pipeline

A WAFER pipeline is one TOML file passed to `wafer --config <path>`. This
how-to covers the common configuration tasks. Every key, type and default is in
the [config schema reference](../interfaces/config-schema.md); start from a
file under `examples/`, such as `examples/dag-passthrough.toml`.

## Connect nodes into a pipeline

Declare each node as `[nodes.<id>]` with a `type`, then connect them with
`[[edges]]`. Processing nodes (`transform`, `filter`, `router`) take a
`plugin`: a path to a `.wasm` component, relative to the config file, or an OCI
reference. Sources and sinks take a `kind`.

```toml
[nodes.mqtt-in]
type   = "source"
kind   = "mqtt"
broker = "localhost"
topic  = "sensors/#"

[nodes.threshold]
type   = "filter"
plugin = "../plugins/threshold-filter/target/wasm32-wasip2/release/wafer_threshold_filter.wasm"

[nodes.threshold.config]          # passed to the plugin as JSON
field = "temperature"
min   = 0.0
max   = 40.0

[nodes.route]
type   = "router"
plugin = "../plugins/content-router/target/wasm32-wasip2/release/wafer_content_router.wasm"

[nodes.route.config]
route_field  = "level"
default_port = "log"
routes       = { alert = "alert" }

[nodes.alert-sink]
type   = "sink"
kind   = "mqtt"
broker = "localhost"
topic  = "alerts"

[nodes.log-sink]
type = "sink"
kind = "stdout"

[[edges]]
from = "mqtt-in"
to   = "threshold"

[[edges]]
from = "threshold"
to   = "route"

[[edges]]
from = "route"
to   = "alert-sink"
port = "alert"                    # router output port

[[edges]]
from = "route"
to   = "log-sink"
port = "log"
```

Sources, Transforms and Filters send every message to all their outgoing
edges; a Router sends it only to the edges whose `port` the plugin returned.

## Size queues and choose what happens when one is full

Set `capacity` and `overflow` on the edge into a node:

```toml
[[edges]]
from     = "route"
to       = "log-sink"
port     = "log"
capacity = 4096
overflow = "dead-letter"          # slow (default) | drop | dead-letter
```

A node has one input queue however many edges reach it; its size is the
largest `capacity` among those edges, or `[engine].default_queue_capacity`.

## Bound plugin execution time and memory

```toml
[engine]
epoch_deadline = 100              # ticks per Wasm call
epoch_tick_ms  = 10               # so each call is capped at about 1 s

[engine.fuel]
transform = 10_000_000
filter    = 500_000
router    = 500_000

[nodes.threshold]
# ...
fuel         = 1_000_000          # overrides [engine.fuel] for this node
memory_limit = 8_388_608          # bytes; overrides [engine.memory]
```

Fuel and epoch limits are off unless you set them. A call that exceeds either
follows the `timed_out` error policy.

## Decide what happens when a plugin fails

`[error_policy]` maps each error category a plugin can return to `skip`,
`dlq` or `teardown`, with retries for transient failures:

```toml
[error_policy]
bad_input = "skip"

[error_policy.dependency_failed]
retries    = 5
backoff_ms = 200
exhausted  = "dlq"
```

A `[nodes.<id>.error_policy]` table replaces the whole pipeline table for that
node; keys it omits take the built-in defaults, not the pipeline values. Other
traps and `unrecoverable` errors are not configurable: the message goes to the
dead-letter queue and the node restarts from its cached component.

## Send failed messages to a dead-letter queue

```toml
[dead_letter]
kind = "file"                     # or "mqtt" with broker, port, topic
path = "dlq.jsonl"
```

The default error policy sends to the dead-letter queue, so a pipeline with a
processing node needs this section unless every `dlq` action is changed to
`skip` or `teardown`. A relative `path` is resolved against the working
directory.

## Let a plugin call an HTTP endpoint

Outbound HTTP is denied unless the node grants each destination:

```toml
[nodes.enrich.capabilities]
outbound_http = [
  { scheme = "https", host = "api.example.com" },
  { scheme = "http", host = "127.0.0.1", port = 8080 },
]
```

The grant lasts until the pipeline restarts. The matching rules are in the
[reference](../interfaces/config-schema.md#outbound-wasihttp-capability) and
[ADR-0016](../adr/0016-outbound-wasi-http-capability.md).

## Change the control-plane address

The API and `/metrics` listen on `127.0.0.1:9090` by default:

```toml
[api]
bind = "0.0.0.0:9090"
```

`--api-bind`, `--no-api` and `--metrics-bind` override this at start-up; see
[observability](observability.md) and the [HTTP API](../interfaces/http-api.md).

## Load plugins from a registry

Write an OCI reference in `plugin` and, optionally, set the cache directory:

```toml
[registry]
cache_dir = "/var/cache/wafer/oci"
```

See [registry.md](registry.md).

## Check a config

There is no separate check command: start the runtime. It rejects unknown keys
while parsing, then runs every [validation](../interfaces/config-schema.md#validation)
check before loading plugins, prints all failures together and exits with
status 2.
