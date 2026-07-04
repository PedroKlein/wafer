---
name: mqtt-iot
description: >
  MQTT protocol patterns for WAFER's IoT edge pipeline using rumqttc. Covers eventloop
  architecture and why polling must never stop, cancel-safe message bridging to the
  pipeline, at-least-once delivery with idempotent dedup, WAL-based persistence for
  patchy connectivity, TLS/mTLS configuration for production, topic isolation as failure
  boundaries, and client lifecycle with tokio. Use when working with MQTT source/sink
  nodes, configuring broker connections, handling reconnection, debugging message delivery,
  or hardening for production edge deployment. Triggers on: MQTT, rumqttc, broker, topic,
  subscribe, publish, QoS, eventloop, MqttOptions, AsyncClient, mosquitto, retain,
  clean session, keep alive, connection, IoT, TLS, mTLS, edge. Do NOT use for general
  async patterns (use async-tokio) or queue/backpressure design (use dag-orchestration).
---

# MQTT/IoT Protocol Patterns

## Core Architecture: rumqttc's Split Model

rumqttc has a unique architecture that catches every newcomer:
- **`AsyncClient`** — cloneable handle for publish/subscribe/unsubscribe
- **`EventLoop`** — owns the TCP connection; MUST be polled continuously

```rust
let (client, eventloop) = AsyncClient::new(options, /* request_channel_capacity */ 10);
```

The second parameter is the internal request channel from Client → EventLoop.
This IS backpressure: if the eventloop can't keep up, `client.publish().await` blocks.

**The eventloop polling rule**: If you stop calling `eventloop.poll().await`, the
MQTT connection dies silently. No error is returned to you. The broker disconnects
after keep_alive timeout expires because it receives no PING. Messages are lost.

---

## WAFER's MQTT Source: Bridging to Pipeline

The source spawns a dedicated task for the eventloop, bridging messages into the
pipeline via an internal `mpsc` channel:

```rust
tokio::spawn(async move {
    let mut is_connected = false;
    loop {
        match eventloop.poll().await {
            Ok(Event::Incoming(Packet::Publish(msg))) => {
                // Backpressure: blocks eventloop if pipeline is saturated
                // This is INTENTIONAL — slows MQTT consumption when pipeline is slow
                if tx.send(msg).await.is_err() {
                    break;  // Receiver dropped = pipeline shutting down
                }
            }
            Ok(Event::Incoming(Packet::ConnAck(_))) => {
                // CRITICAL: Re-subscribe on every reconnection
                client.subscribe(&topic, qos).await.ok();
                is_connected = true;
            }
            Err(e) => {
                is_connected = false;
                // rumqttc reconnects internally on next poll()
                // Sleep prevents tight error loop consuming CPU
                tracing::warn!(error = %e, "MQTT connection error");
                tokio::time::sleep(Duration::from_secs(1)).await;
            }
            _ => {}  // PingResp, SubAck, PubAck — ignore
        }
    }
});
```

**The re-subscribe bug**: With `clean_session: true`, the broker forgets all
subscriptions on disconnect. After reconnection (ConnAck), you MUST re-subscribe.
Without this, the source appears connected but receives zero messages. This is the
#1 rumqttc production bug — it looks like a broker problem but it's client-side.

---

## Delivery Semantics for Edge Pipelines

### At-Least-Once + Idempotent Processing

The standard pattern for reliable edge data pipelines (from RelayQ, edge-convoy,
Azure IoT Edge production guidance):

1. **Source**: QoS 1 (at-least-once) + `clean_session: false` for durable subscriptions
2. **Pipeline**: Idempotent transforms (same input → same output regardless of replay)
3. **Dedup**: By envelope ID at pipeline boundary if exactly-once semantics needed
4. **Sink**: QoS 1 for reliable forwarding, QoS 0 only for high-frequency telemetry

### Why Not QoS 2 (Exactly-Once)?

- 4-step handshake (PUBLISH → PUBREC → PUBREL → PUBCOMP) halves throughput
- Double the round-trip latency per message
- Broker must maintain per-message state until completion
- **Idempotent processing + QoS 1 gives the same effective guarantee at 2x throughput**

### Durable Subscriptions for Crash Recovery

```rust
options.set_clean_session(false);  // Broker remembers subscriptions across reconnects
// Combined with QoS 1: messages during disconnect are queued by broker
// On reconnect: broker redelivers all unacked messages (may duplicate!)
```

---

## Connection Configuration for Edge

```rust
let mut options = MqttOptions::new(
    format!("wafer-{pipeline}-{node_id}"),  // Deterministic, unique client ID
    broker_host,
    broker_port
);

// Keep alive: broker disconnects if no PING within 1.5x this interval
// MUST be longer than your longest WASM processing time + pipeline backpressure delay
options.set_keep_alive(Duration::from_secs(30));

// Inflight: max unacknowledged QoS 1/2 messages in transit
// Higher = more throughput, but more redelivery on failure
options.set_inflight(10);

// Clean session: false = durable subscription (survives restart)
options.set_clean_session(false);
```

**Client ID collision**: Two clients with the same ID = connection war. The broker
disconnects the older client. Both sides reconnect, kick each other, loop forever.
Use deterministic IDs: `wafer-{pipeline_name}-{node_id}`.

**Keep alive vs processing time**: If your WASM transform takes 2s and the eventloop
task is blocked on `tx.send().await` (backpressure), the eventloop can't send PING.
If keep_alive is 5s, one slow message + backpressure = disconnect. Set keep_alive to
at least 3x your worst-case pipeline latency.

---

## Production Hardening

### TLS/mTLS for Non-Local Brokers

```rust
use rumqttc::Transport;
use tokio_rustls::rustls::{ClientConfig, RootCertStore};

let mut root_store = RootCertStore::empty();
root_store.add_parsable_certificates(ca_certs);

let tls_config = ClientConfig::builder()
    .with_root_certificates(root_store)
    .with_client_auth_cert(client_cert_chain, client_key)?;  // mTLS

options.set_transport(Transport::tls_with_config(tls_config.into()));
```

### WAL-Based Persistence for Patchy Connectivity

For edge deployments with unreliable networks (satellite, cellular, field sensors):
```
MQTT Source → WAL (SQLite/append-only file) → Pipeline → Sink
```

Pattern from edge-convoy and industrial MQTT bridges:
- Write received messages to local WAL before pipeline processing
- On pipeline crash/restart: replay from WAL position, not from broker
- Broker retention is limited; local WAL gives indefinite recovery window

### Topic Isolation as Failure Boundary

```
sensors/{device_type}/{device_id}/telemetry  # High-frequency, loss OK
commands/{device_id}/+                       # Low-frequency, must deliver
alerts/{severity}/#                          # Critical, durable subscription
```

A misbehaving device flooding `sensors/+/+/telemetry` must not affect command delivery.
Use separate MQTT clients (separate eventloops) for critical vs best-effort topics.

---

## MQTT Sink Pattern

```rust
async fn emit(&mut self, envelope: &RuntimeEnvelope) -> Result<()> {
    self.client.publish(&self.topic, self.qos, /* retain */ false, &envelope.payload)
        .await
        .map_err(|e| WaferError::SinkWrite { message: e.to_string() })?;
    Ok(())
}
```

**Retain flag**: Only for "last known value" (device status, config).
Never retain high-frequency streams — fills broker storage and confuses new subscribers
who get stale retained messages on first connect.

---

## NEVER

- **NEVER stop polling the eventloop** — the MQTT connection dies silently; no error
  is returned; broker disconnects after keep_alive expiry; messages vanish
- **NEVER forget to re-subscribe after ConnAck** — with `clean_session: true`, broker
  drops subscriptions on disconnect; source goes silent without error
- **NEVER use the same client ID for multiple pipeline instances** — connection war:
  both clients reconnect and kick each other in an infinite loop
- **NEVER set keep_alive shorter than worst-case pipeline backpressure** — if the
  eventloop can't send PING because `tx.send().await` is blocked, broker kills you
- **NEVER assume messages arrive exactly once with QoS 1** — redelivery after reconnection
  is normal behavior; design transforms to be idempotent
- **NEVER subscribe to `#` in production** — receives EVERY message on the broker;
  overwhelms the pipeline with irrelevant traffic; use scoped topic prefixes
- **NEVER use QoS 2 for sensor telemetry** — 4-step handshake halves throughput for
  data that's inherently time-series (superseded by next reading anyway)
- **NEVER run production MQTT over plaintext** — any network hop beyond localhost
  requires TLS at minimum; mTLS for device authentication in industrial deployments
