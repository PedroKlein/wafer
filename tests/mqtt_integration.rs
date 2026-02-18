//! Integration tests for MQTT Source and Sink nodes.
//!
//! Uses testcontainers with Mosquitto for ephemeral MQTT broker instances.

use std::time::Duration;

use rumqttc::{AsyncClient, Event, MqttOptions, Packet, QoS};
use testcontainers::runners::AsyncRunner;
use testcontainers_modules::mosquitto::Mosquitto;
use tokio::task::JoinHandle;
use tokio::time::timeout;
use uuid::Uuid;
use wafer_poc::node::{Lifecycle, MqttSink, MqttSource, Sink, Source};
use wafer_poc::queue::RuntimeEnvelope;

/// Generate a unique client ID to avoid MQTT broker conflicts.
fn unique_client_id(prefix: &str) -> String {
    format!("{}-{}", prefix, Uuid::new_v4())
}

/// Spawn a background task to poll the MQTT eventloop.
/// This is required for rumqttc clients to process incoming/outgoing packets.
fn spawn_eventloop_task(mut eventloop: rumqttc::EventLoop) -> JoinHandle<()> {
    tokio::spawn(async move {
        loop {
            if eventloop.poll().await.is_err() {
                tokio::time::sleep(Duration::from_millis(50)).await;
            }
        }
    })
}

/// Helper to publish repeatedly until MqttSource receives (handles subscription race).
async fn publish_until_received(
    publisher: &AsyncClient,
    source: &mut MqttSource,
    topic: &str,
    payload: &[u8],
    timeout_secs: u64,
) -> RuntimeEnvelope {
    let publish_task = async {
        loop {
            let _ = publisher
                .publish(topic, QoS::AtLeastOnce, false, payload.to_vec())
                .await;
            tokio::time::sleep(Duration::from_millis(200)).await;
        }
    };

    let receive_task = async {
        loop {
            if let Ok(Some(envelope)) = source.poll().await {
                return envelope;
            }
        }
    };

    tokio::select! {
        envelope = receive_task => envelope,
        _ = tokio::time::sleep(Duration::from_secs(timeout_secs)) => {
            panic!("Timeout: source never received message after {}s", timeout_secs);
        }
        _ = publish_task => unreachable!(),
    }
}

/// Test that MqttSource can receive a message published to its subscribed topic.
#[tokio::test]
async fn test_mqtt_source_receives_message() {
    // 1. Start Mosquitto container
    let container = Mosquitto::default()
        .start()
        .await
        .expect("Failed to start Mosquitto container");

    let host = container.get_host().await.expect("Failed to get host");
    let port = container
        .get_host_port_ipv4(1883)
        .await
        .expect("Failed to get port");

    let topic = "test/source/topic";
    let test_payload = b"hello from mqtt integration test";

    // 2. Create and init MqttSource subscribed to topic
    let mut source = MqttSource::new(
        "test-source",
        host.to_string(),
        port,
        topic,
        1, // QoS 1 (AtLeastOnce)
        unique_client_id("source"),
    );

    source.validate().expect("Source validation failed");
    source.init().await.expect("Source init failed");

    // 3. Create publisher client
    let mut pub_options = MqttOptions::new(unique_client_id("publisher"), host.to_string(), port);
    pub_options.set_keep_alive(Duration::from_secs(30));

    let (pub_client, pub_eventloop) = AsyncClient::new(pub_options, 10);
    let pub_handle = spawn_eventloop_task(pub_eventloop);

    // Small delay to let publisher connect
    tokio::time::sleep(Duration::from_millis(100)).await;

    // 4. Publish repeatedly until source receives (handles subscription race)
    let envelope = publish_until_received(&pub_client, &mut source, topic, test_payload, 5).await;

    assert_eq!(
        envelope.payload, test_payload,
        "Payload mismatch: expected {:?}, got {:?}",
        test_payload, envelope.payload
    );

    let source_route = envelope
        .metadata
        .get("source_route")
        .expect("Expected source_route metadata");
    assert_eq!(
        source_route, topic,
        "source_route metadata mismatch: expected {}, got {}",
        topic, source_route
    );

    // 6. Cleanup
    pub_handle.abort();
    let _ = pub_client.disconnect().await;
    source.close().await.expect("Source close failed");
}

/// Test that MqttSink can publish a message to the broker.
#[tokio::test]
async fn test_mqtt_sink_publishes_message() {
    // 1. Start Mosquitto container
    let container = Mosquitto::default()
        .start()
        .await
        .expect("Failed to start Mosquitto container");

    let host = container.get_host().await.expect("Failed to get host");
    let port = container
        .get_host_port_ipv4(1883)
        .await
        .expect("Failed to get port");

    let topic = "test/sink/topic";
    let test_payload = b"hello from sink test";

    // 2. Create subscriber client on topic
    let mut sub_options = MqttOptions::new(unique_client_id("subscriber"), host.to_string(), port);
    sub_options.set_keep_alive(Duration::from_secs(30));

    let (sub_client, mut sub_eventloop) = AsyncClient::new(sub_options, 10);

    // Subscribe to topic
    sub_client
        .subscribe(topic, QoS::AtLeastOnce)
        .await
        .expect("Failed to subscribe");

    // Wait for SubAck (handle ConnAck and other packets first)
    let mut subscribed = false;
    for _ in 0..20 {
        match sub_eventloop.poll().await {
            Ok(Event::Incoming(Packet::SubAck(_))) => {
                subscribed = true;
                break;
            }
            Ok(_) => continue,
            Err(_) => {
                tokio::time::sleep(Duration::from_millis(50)).await;
            }
        }
    }
    assert!(subscribed, "Did not receive SubAck");

    // 3. Create and init MqttSink
    let mut sink = MqttSink::new(
        "test-sink",
        host.to_string(),
        port,
        topic,
        1, // QoS 1 (AtLeastOnce)
        unique_client_id("sink"),
    );

    sink.validate().expect("Sink validation failed");
    sink.init().await.expect("Sink init failed");

    // Small delay to let sink connect
    tokio::time::sleep(Duration::from_millis(100)).await;

    // 4. Call collect() with envelope
    let envelope = RuntimeEnvelope::new("test-source", test_payload.to_vec());
    sink.collect(envelope)
        .await
        .expect("Sink collect failed");

    // 5. Assert subscriber receives the message
    let receive_result = timeout(Duration::from_secs(5), async {
        loop {
            match sub_eventloop.poll().await {
                Ok(Event::Incoming(Packet::Publish(publish))) => {
                    return Some(publish);
                }
                Ok(_) => continue,
                Err(e) => {
                    eprintln!("Eventloop error: {}", e);
                    tokio::time::sleep(Duration::from_millis(50)).await;
                }
            }
        }
    })
    .await
    .expect("Timeout waiting for message from sink");

    let publish = receive_result.expect("Expected Some(publish), got None");

    assert_eq!(
        publish.payload.to_vec(),
        test_payload.to_vec(),
        "Received payload mismatch"
    );
    assert_eq!(publish.topic, topic, "Topic mismatch");

    // 6. Cleanup
    let _ = sub_client.disconnect().await;
    sink.close().await.expect("Sink close failed");
}

/// End-to-end test: MqttSource and MqttSink working together.
/// MqttSink publishes a message, MqttSource (subscribed to same topic) receives it.
#[tokio::test]
async fn test_mqtt_source_and_sink_integration() {
    // 1. Start Mosquitto container
    let container = Mosquitto::default()
        .start()
        .await
        .expect("Failed to start Mosquitto container");

    let host = container.get_host().await.expect("Failed to get host");
    let port = container
        .get_host_port_ipv4(1883)
        .await
        .expect("Failed to get port");

    let topic = "test/e2e/topic";
    let test_payload = b"end-to-end integration test message";

    // 2. Create and init MqttSource subscribed to topic
    let mut source = MqttSource::new(
        "e2e-source",
        host.to_string(),
        port,
        topic,
        1,
        unique_client_id("e2e-source"),
    );

    source.validate().expect("Source validation failed");
    source.init().await.expect("Source init failed");

    // 3. Create and init MqttSink publishing to the same topic
    let mut sink = MqttSink::new(
        "e2e-sink",
        host.to_string(),
        port,
        topic,
        1,
        unique_client_id("e2e-sink"),
    );

    sink.validate().expect("Sink validation failed");
    sink.init().await.expect("Sink init failed");

    // Small delay to let sink connect
    tokio::time::sleep(Duration::from_millis(100)).await;

    // 4. Publish via MqttSink repeatedly until source receives (handles subscription race)
    let received_envelope = {
        let publish_task = async {
            loop {
                let envelope = RuntimeEnvelope::new("e2e-test", test_payload.to_vec());
                let _ = sink.collect(envelope).await;
                tokio::time::sleep(Duration::from_millis(200)).await;
            }
        };

        let receive_task = async {
            loop {
                if let Ok(Some(envelope)) = source.poll().await {
                    return envelope;
                }
            }
        };

        tokio::select! {
            envelope = receive_task => envelope,
            _ = tokio::time::sleep(Duration::from_secs(5)) => {
                panic!("Timeout: source never received message after 5s");
            }
            _ = publish_task => unreachable!(),
        }
    };

    assert_eq!(
        received_envelope.payload, test_payload,
        "E2E payload mismatch"
    );

    let source_route = received_envelope
        .metadata
        .get("source_route")
        .expect("Expected source_route metadata");
    assert_eq!(source_route, topic, "E2E source_route mismatch");

    // 6. Cleanup
    source.close().await.expect("Source close failed");
    sink.close().await.expect("Sink close failed");
}
