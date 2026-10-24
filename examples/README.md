# Example Pipeline Configurations

These TOML files are runnable pipeline configurations for the `wafer` binary.
Relative plugin paths resolve against the file's directory. For every key, see
[`docs/interfaces/config-schema.md`](../docs/interfaces/config-schema.md) and
[`docs/rfcs/RFC-004-config-schema.md`](../docs/rfcs/RFC-004-config-schema.md).

## Examples

| File | Pipeline |
|---|---|
| `dag-passthrough.toml` | stdin, pass-through transform, stdout (the minimal pipeline) |
| `dag-uppercase.toml` | stdin, uppercase transform, stdout |
| `dag-json-parse.toml` | stdin, json-parse transform, stdout |
| `dag-chain.toml` | stdin, json-parse transform, threshold filter, stdout |
| `dag-filter.toml` | stdin, threshold filter, stdout |
| `dag-fanout.toml` | stdin, content router, two file sinks |
| `dag-file-io.toml` | file source, pass-through, file sink |
| `dag-mqtt.toml`, `dag-mqtt-simple.toml` | MQTT source and sink, with and without a pass-through transform |
| `dag-http.toml` | HTTP webhook source, pass-through, HTTP sink |
| `dag-overflow-dlq-demo.toml` | file source, delayed transform behind a capacity-1 edge that overflows to the dead-letter queue, file sink |
| `dag-metrics-demo.toml` | file pipeline with the control plane and metrics enabled |
| `dag-mnist-inference.toml` | MNIST inference on CPU (see below) |
| `dag-passthrough-with-api.toml` | pass-through pipeline with the control plane enabled |

The comment at the top of each file names the plugins to build first and, where the input is simple, the command that runs it.

`dag-mnist-inference.toml` sets `allow_inference = true` on its Wasm Transform;
build the MNIST and result-format components with the commands in the file
before running it. It demonstrates that the inference path works, not
inference performance or CUDA stability.

`prometheus.yml` is a Prometheus scrape configuration for `localhost:9091`,
for a runtime started with `dag-passthrough-with-api.toml` and
`--metrics-bind 127.0.0.1:9091`.
