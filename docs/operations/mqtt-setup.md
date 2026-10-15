# Run the MQTT examples against a local broker

This guide starts a Mosquitto broker in Docker and runs the MQTT example
pipelines against it. The MQTT source and sink connect without TLS or
credentials, so the broker must accept anonymous clients on plain TCP.

## Prerequisites

- Docker, installed and running.
- The plugins built as in [Getting Started](getting-started.md) (only
  `examples/dag-mqtt.toml` needs one: `pass-through`).
- Optional: `mosquitto_pub` and `mosquitto_sub` (`mosquitto-clients` on
  Debian/Ubuntu, `brew install mosquitto` on macOS).

## Start the broker

From the repository root, which holds the `mosquitto.conf` used here
(anonymous access on port 1883):

```bash
docker run -d --name mosquitto -p 1883:1883 \
  -v "$(pwd)/mosquitto.conf:/mosquitto/config/mosquitto.conf" \
  eclipse-mosquitto:2
docker ps --filter name=mosquitto
```

## Run an MQTT pipeline

`examples/dag-mqtt.toml` subscribes to `input/messages`, passes each message
through the `pass-through` plugin, and publishes it to `output/processed`.
`examples/dag-mqtt-simple.toml` does the same without a Wasm node.

```bash
cargo run -p wafer-runtime -- --config examples/dag-mqtt.toml
```

In a second terminal, watch the output topic:

```bash
mosquitto_sub -h localhost -t output/processed
```

In a third, publish a message:

```bash
mosquitto_pub -h localhost -t input/messages -m "Hello MQTT"
```

The subscriber prints `Hello MQTT`. Stop the pipeline with `Ctrl-C`.

To point a pipeline at another broker or port, set `broker` and `port` on the
MQTT source and sink; see the
[source and sink reference](../interfaces/config-schema.md#source-and-sink).

## Clean up

```bash
docker rm -f mosquitto
```

## Troubleshooting

- **Connection refused**: check that the container is running
  (`docker ps --filter name=mosquitto`) and read `docker logs mosquitto`.
- **Port 1883 already in use**: find the other process with
  `lsof -i :1883`, or map another host port (`-p 11883:1883`) and set
  `port = 11883` on the pipeline's MQTT nodes.
