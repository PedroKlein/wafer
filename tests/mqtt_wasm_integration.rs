//! Integration tests for MQTT Source → WASM Transform pipeline.
//!
//! Tests the complete flow of receiving messages via MQTT and processing them
//! through a WASM transform node. Uses testcontainers with Mosquitto for
//! ephemeral MQTT broker instances.

use std::path::PathBuf;
use std::time::Duration;

use rumqttc::{AsyncClient, MqttOptions, QoS};
use testcontainers::runners::AsyncRunner;
use testcontainers_modules::mosquitto::Mosquitto;
use tokio::task::JoinHandle;
use uuid::Uuid;

use wafer_poc::engine::{Capabilities, TransformInstance, WaferEngine};
use wafer_poc::node::{Lifecycle, MqttSource, NodeConfig, ProcessResult, Source, Transform, WasmTransform};

/// Get project root directory.
fn project_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

/// Get path to pass-through plugin.
fn plugin_path() -> PathBuf {
    project_root()
        .join("plugins/pass-through/target/wasm32-wasip2/release/pass_through_transform.wasm")
}

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

/// Create a WASM transform with the pass-through plugin.
async fn create_wasm_transform(engine: &WaferEngine, id: &str) -> WasmTransform {
    let component = engine
        .load_component(plugin_path())
        .expect("Failed to load plugin");
    let instance = TransformInstance::new(engine, &component, Capabilities::with_stdio())
        .await
        .expect("Failed to create instance");
    let config = NodeConfig::new(id, "transform/passthrough");
    WasmTransform::new(
        WaferEngine::new().expect("Failed to create engine for transform"),
        instance,
        config,
    )
}

/// Test MQTT → WASM transform pipeline with multiple messages.
///
/// This test validates that:
/// 1. Messages received from MQTT source can be processed through WASM transform
/// 2. Multiple messages (10) are all processed correctly
/// 3. The epoch ticker is properly started and cleaned up
///
/// This test is expected to FAIL before the epoch fix is applied.
/// After Task 4, the `#[ignore]` attribute should be removed.
#[tokio::test]
async fn test_mqtt_wasm_multiple_messages() {
    // 1. Skip if plugin not built
    if !plugin_path().exists() {
        eprintln!(
            "Skipping test: pass-through plugin not built at {:?}",
            plugin_path()
        );
        return;
    }

    // 2. Start Mosquitto container
    let container = Mosquitto::default()
        .start()
        .await
        .expect("Failed to start Mosquitto container");

    let host = container.get_host().await.expect("Failed to get host");
    let port = container
        .get_host_port_ipv4(1883)
        .await
        .expect("Failed to get port");

    let topic = "test/mqtt-wasm/input";
    let message_count = 10;

    // 3. Create WaferEngine, load component, create TransformInstance
    let engine = WaferEngine::new().expect("Failed to create engine");

    // 4. Start epoch ticker - CRITICAL for WASM execution
    let epoch_ticker = engine.start_epoch_ticker();

    // 5. Create MqttSource subscribed to input topic
    let mut source = MqttSource::new(
        "mqtt-source",
        host.to_string(),
        port,
        topic,
        1, // QoS 1 (AtLeastOnce)
        unique_client_id("mqtt-wasm-source"),
    );

    source.validate().expect("Source validation failed");
    source.init().await.expect("Source init failed");

    // 6. Create and init transform
    let mut transform = create_wasm_transform(&engine, "wasm-transform").await;
    transform.init().await.expect("Transform init failed");

    // 7. Create publisher client
    let mut pub_options = MqttOptions::new(unique_client_id("publisher"), host.to_string(), port);
    pub_options.set_keep_alive(Duration::from_secs(30));

    let (pub_client, pub_eventloop) = AsyncClient::new(pub_options, 10);
    let pub_handle = spawn_eventloop_task(pub_eventloop);

    // Small delay to let publisher connect
    tokio::time::sleep(Duration::from_millis(100)).await;

    // 8. Send messages
    for i in 0..message_count {
        let payload = format!("mqtt-wasm-message-{}", i);
        pub_client
            .publish(topic, QoS::AtLeastOnce, false, payload.as_bytes().to_vec())
            .await
            .expect("Failed to publish message");
    }

    // 9. Poll source → process through transform → verify no errors
    let mut processed_count = 0;
    let mut errors = Vec::new();

    let timeout_duration = Duration::from_secs(10);
    let start = std::time::Instant::now();

    while processed_count < message_count && start.elapsed() < timeout_duration {
        match tokio::time::timeout(Duration::from_millis(500), source.poll()).await {
            Ok(Ok(Some(envelope))) => {
                // Process through transform
                match transform.process(envelope).await {
                    Ok(ProcessResult::Emit(output)) => {
                        // Verify output matches input (pass-through)
                        let expected_prefix = "mqtt-wasm-message-";
                        let output_str = String::from_utf8_lossy(&output.payload);
                        if output_str.starts_with(expected_prefix) {
                            processed_count += 1;
                        } else {
                            errors.push(format!(
                                "Unexpected payload: expected prefix '{}', got '{}'",
                                expected_prefix, output_str
                            ));
                        }
                    }
                    Ok(ProcessResult::Filter) => {
                        errors.push("Unexpected Filter result from pass-through transform".to_string());
                    }
                    Ok(ProcessResult::Error(e)) => {
                        errors.push(format!("Transform error: {} (code: {})", e.message, e.code));
                    }
                    Err(e) => {
                        errors.push(format!("Transform execution error: {}", e));
                    }
                }
            }
            Ok(Ok(None)) => {
                // No message available, continue polling
                tokio::time::sleep(Duration::from_millis(50)).await;
            }
            Ok(Err(e)) => {
                errors.push(format!("Source poll error: {}", e));
            }
            Err(_) => {
                // Timeout on individual poll, continue
            }
        }
    }

    // 10. Assert all messages processed
    assert!(
        errors.is_empty(),
        "Errors during processing: {:?}",
        errors
    );
    assert_eq!(
        processed_count, message_count,
        "Expected {} messages processed, got {}",
        message_count, processed_count
    );

    // 11. Cleanup: abort epoch ticker, close nodes
    epoch_ticker.abort();
    pub_handle.abort();
    let _ = pub_client.disconnect().await;
    transform.close().await.expect("Transform close failed");
    source.close().await.expect("Source close failed");
}

/// Test a single message through MQTT → WASM pipeline.
///
/// This is a simpler variant to help isolate issues.
#[tokio::test]
async fn test_mqtt_wasm_single_message() {
    // Skip if plugin not built
    if !plugin_path().exists() {
        eprintln!(
            "Skipping test: pass-through plugin not built at {:?}",
            plugin_path()
        );
        return;
    }

    // Start Mosquitto container
    let container = Mosquitto::default()
        .start()
        .await
        .expect("Failed to start Mosquitto container");

    let host = container.get_host().await.expect("Failed to get host");
    let port = container
        .get_host_port_ipv4(1883)
        .await
        .expect("Failed to get port");

    let topic = "test/mqtt-wasm/single";
    let test_payload = b"hello-from-mqtt-wasm-test";

    // Create engine and start epoch ticker
    let engine = WaferEngine::new().expect("Failed to create engine");
    let epoch_ticker = engine.start_epoch_ticker();

    // Create and init source
    let mut source = MqttSource::new(
        "mqtt-source",
        host.to_string(),
        port,
        topic,
        1,
        unique_client_id("mqtt-wasm-single-source"),
    );
    source.validate().expect("Source validation failed");
    source.init().await.expect("Source init failed");

    // Create and init transform
    let mut transform = create_wasm_transform(&engine, "wasm-transform").await;
    transform.init().await.expect("Transform init failed");

    // Create publisher
    let mut pub_options = MqttOptions::new(unique_client_id("publisher"), host.to_string(), port);
    pub_options.set_keep_alive(Duration::from_secs(30));
    let (pub_client, pub_eventloop) = AsyncClient::new(pub_options, 10);
    let pub_handle = spawn_eventloop_task(pub_eventloop);

    tokio::time::sleep(Duration::from_millis(100)).await;

    // Publish repeatedly until source receives (handles subscription race)
    let envelope = {
        let publish_task = async {
            loop {
                let _ = pub_client
                    .publish(topic, QoS::AtLeastOnce, false, test_payload.to_vec())
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
            _ = tokio::time::sleep(Duration::from_secs(5)) => {
                panic!("Timeout: source never received message after 5s");
            }
            _ = publish_task => unreachable!(),
        }
    };

    // Process through transform
    let result = transform.process(envelope).await.expect("Transform failed");

    match result {
        ProcessResult::Emit(output) => {
            assert_eq!(
                output.payload, test_payload,
                "Payload mismatch: expected {:?}, got {:?}",
                test_payload,
                output.payload
            );
        }
        ProcessResult::Filter => {
            panic!("Unexpected Filter result from pass-through transform");
        }
        ProcessResult::Error(e) => {
            panic!("Transform error: {} (code: {})", e.message, e.code);
        }
    }

    // Cleanup
    epoch_ticker.abort();
    pub_handle.abort();
    let _ = pub_client.disconnect().await;
    transform.close().await.expect("Transform close failed");
    source.close().await.expect("Source close failed");
}
