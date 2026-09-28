#![cfg(test)]
//! The MQTT source must keep receiving after the broker restarts. With a
//! clean session the broker forgets the subscription on disconnect, so the
//! source has to subscribe again on every new connection.
//!
//! Needs a Docker daemon: `mise run test-docker`.

use std::net::TcpListener;
use std::time::Duration;

use rumqttc::{AsyncClient, MqttOptions, QoS};
use testcontainers::core::IntoContainerPort;
use testcontainers::runners::AsyncRunner;
use testcontainers::{ContainerAsync, ImageExt};
use testcontainers_modules::mosquitto::Mosquitto;
use tokio::task::JoinHandle;
use wafer_core::node::{Lifecycle, MqttSource, Source};

const TOPIC: &str = "wafer/reconnect";

/// A restarted container keeps its host port only if the mapping is fixed.
fn free_port() -> u16 {
    TcpListener::bind("127.0.0.1:0").unwrap().local_addr().unwrap().port()
}

fn spawn_publisher(port: u16) -> (AsyncClient, JoinHandle<()>) {
    let options = MqttOptions::new("wafer-reconnect-publisher", "127.0.0.1", port);
    let (client, mut eventloop) = AsyncClient::new(options, 10);
    let handle = tokio::spawn(async move {
        loop {
            if eventloop.poll().await.is_err() {
                tokio::time::sleep(Duration::from_millis(100)).await;
            }
        }
    });
    (client, handle)
}

/// Publishes `payload` until the source yields it, since neither client
/// reports when its (re)subscription reached the broker.
async fn deliver(publisher: &AsyncClient, source: &mut MqttSource, payload: &str) {
    let received = async {
        loop {
            publisher.publish(TOPIC, QoS::AtLeastOnce, false, payload).await.unwrap();
            if let Ok(polled) =
                tokio::time::timeout(Duration::from_millis(250), source.poll()).await
            {
                let envelope = polled.unwrap().expect("MQTT source ended");
                if envelope.payload.as_ref() == payload.as_bytes() {
                    return;
                }
            }
        }
    };
    tokio::time::timeout(Duration::from_secs(20), received)
        .await
        .unwrap_or_else(|_| panic!("MQTT source never received {payload:?}"));
}

async fn start_broker(port: u16) -> ContainerAsync<Mosquitto> {
    Mosquitto::default().with_mapped_port(port, 1883.tcp()).start().await.unwrap()
}

#[tokio::test]
#[ignore = "needs a Docker daemon: mise run test-docker"]
async fn mqtt_source_keeps_receiving_after_broker_restart() {
    let port = free_port();
    let broker = start_broker(port).await;

    let mut source =
        MqttSource::new("mqtt-src", "127.0.0.1", port, TOPIC, 1, "wafer-reconnect-source");
    source.init().await.unwrap();
    let (publisher, publisher_loop) = spawn_publisher(port);

    deliver(&publisher, &mut source, "before-restart").await;

    broker.stop_with_timeout(Some(0)).await.unwrap();
    broker.start().await.unwrap();

    deliver(&publisher, &mut source, "after-restart").await;

    publisher_loop.abort();
    source.close().await.unwrap();
}
