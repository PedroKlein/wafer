# Local MQTT Development Setup

This guide covers setting up a local MQTT broker using Docker for development and testing.

## Prerequisites

- Docker installed and running
- (Optional) `mosquitto-clients` package for testing with CLI tools

## Quick Start

Start the Mosquitto MQTT broker with Docker:

```bash
docker run -d --name mosquitto -p 1883:1883 \
  -v $(pwd)/mosquitto.conf:/mosquitto/config/mosquitto.conf \
  eclipse-mosquitto:2
```

This command:
- Runs Mosquitto in detached mode (`-d`)
- Names the container `mosquitto` for easy management
- Maps port 1883 (standard MQTT port)
- Mounts the local `mosquitto.conf` configuration file

Verify it's running:

```bash
docker ps | grep mosquitto
```

## Testing with CLI Tools

Install the mosquitto clients (macOS):

```bash
brew install mosquitto
```

### Subscribe to a Topic

Open a terminal and subscribe to a test topic:

```bash
mosquitto_sub -h localhost -t "test/topic"
```

This will block and wait for messages.

### Publish to a Topic

In another terminal, publish a message:

```bash
mosquitto_pub -h localhost -t "test/topic" -m "Hello MQTT"
```

You should see "Hello MQTT" appear in the subscriber terminal.

## Connecting from Rust with rumqttc

Add `rumqttc` to your `Cargo.toml`:

```toml
[dependencies]
rumqttc = "0.24"
tokio = { version = "1", features = ["full"] }
```

Example subscriber:

```rust
use rumqttc::{AsyncClient, MqttOptions, QoS};
use std::time::Duration;

#[tokio::main]
async fn main() {
    let mut mqttoptions = MqttOptions::new("rust-client", "localhost", 1883);
    mqttoptions.set_keep_alive(Duration::from_secs(5));

    let (client, mut eventloop) = AsyncClient::new(mqttoptions, 10);
    
    // Subscribe to a topic
    client.subscribe("test/topic", QoS::AtMostOnce).await.unwrap();

    // Process incoming messages
    loop {
        match eventloop.poll().await {
            Ok(notification) => {
                println!("Received: {:?}", notification);
            }
            Err(e) => {
                eprintln!("Error: {:?}", e);
                break;
            }
        }
    }
}
```

Example publisher:

```rust
use rumqttc::{AsyncClient, MqttOptions, QoS};
use std::time::Duration;

#[tokio::main]
async fn main() {
    let mut mqttoptions = MqttOptions::new("rust-publisher", "localhost", 1883);
    mqttoptions.set_keep_alive(Duration::from_secs(5));

    let (client, mut eventloop) = AsyncClient::new(mqttoptions, 10);

    // Spawn task to drive the event loop
    tokio::spawn(async move {
        loop {
            let _ = eventloop.poll().await;
        }
    });

    // Publish a message
    client
        .publish("test/topic", QoS::AtLeastOnce, false, "Hello from Rust!")
        .await
        .unwrap();

    // Give time for message to be sent
    tokio::time::sleep(Duration::from_secs(1)).await;
}
```

## Cleanup

Stop and remove the container:

```bash
docker stop mosquitto
docker rm mosquitto
```

Or remove it in one command:

```bash
docker rm -f mosquitto
```

## Troubleshooting

### Connection refused

Ensure the container is running:

```bash
docker ps | grep mosquitto
```

Check container logs:

```bash
docker logs mosquitto
```

### Port already in use

Check if another process is using port 1883:

```bash
lsof -i :1883
```

Stop any conflicting service or use a different port:

```bash
docker run -d --name mosquitto -p 11883:1883 \
  -v $(pwd)/mosquitto.conf:/mosquitto/config/mosquitto.conf \
  eclipse-mosquitto:2
```

Then connect using port 11883 instead.
