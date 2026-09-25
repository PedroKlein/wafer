# Example Pipeline Configurations

These TOML files are runnable examples for the **current runtime binary**.

The examples use the map-keyed schema loaded by `wafer-config`. For the complete
operator-facing reference, see
[`docs/interfaces/config-schema.md`](../docs/interfaces/config-schema.md) and
[`docs/rfcs/RFC-004-config-schema.md`](../docs/rfcs/RFC-004-config-schema.md).

## Current examples

- `dag-passthrough.toml` — minimal stdin → pass-through transform → stdout.
- `dag-uppercase.toml` — stdin → uppercase transform → stdout.
- `dag-chain.toml` — multi-stage transform chain.
- `dag-filter.toml` — filter example using the current runtime schema.
- `dag-fanout.toml` — router fan-out example.
- `dag-file-io.toml` — file source/sink example.
- `dag-mqtt.toml` / `dag-mqtt-simple.toml` — MQTT source/sink examples.
- `dag-http.toml` — HTTP webhook source/sink example.
- `dag-overflow-dlq-demo.toml` — overflow / DLQ demonstration.
- `dag-metrics-demo.toml` — metrics-enabled pipeline.
- `dag-mnist-inference.toml` — CPU-default MNIST architecture-validation path.
  Its Wasm Transform sets `allow_inference = true`; build the MNIST and
  result-format components with the commands in the file before running it.
  This example demonstrates secondary architecture validation, not inference
  performance or CUDA stability.
- `dag-remote.toml` — OCI/remote plugin reference example.
- `dag-passthrough-with-api.toml` — passthrough pipeline with API settings.

## Removed stale example

`dag-diamond.toml` was removed during the documentation cleanup because it used
the deleted Joiner node / `merge-joiner` plugin. Fan-in is now implicit host
topology (multi-producer mpsc into one consumer); there is no first-class Joiner
node type.
