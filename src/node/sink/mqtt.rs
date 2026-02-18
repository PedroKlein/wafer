//! MQTT-based sink implementation using rumqttc.

use std::future::Future;
use std::pin::Pin;
use std::time::Duration;

use rumqttc::{AsyncClient, EventLoop, MqttOptions, QoS};
use tokio::task::JoinHandle;

use crate::error::{ConfigError, Result, WaferError};
use crate::node::Lifecycle;
use crate::queue::RuntimeEnvelope;

use super::Sink;

pub struct MqttSink {
    id: String,
    broker: String,
    port: u16,
    topic: String,
    qos: QoS,
    client_id: String,
    client: Option<AsyncClient>,
    eventloop_handle: Option<JoinHandle<()>>,
}

impl MqttSink {
    #[must_use]
    pub fn new(
        id: impl Into<String>,
        broker: impl Into<String>,
        port: u16,
        topic: impl Into<String>,
        qos: u8,
        client_id: impl Into<String>,
    ) -> Self {
        let qos = match qos {
            0 => QoS::AtMostOnce,
            1 => QoS::AtLeastOnce,
            _ => QoS::ExactlyOnce,
        };

        Self {
            id: id.into(),
            broker: broker.into(),
            port,
            topic: topic.into(),
            qos,
            client_id: client_id.into(),
            client: None,
            eventloop_handle: None,
        }
    }
}

impl Lifecycle for MqttSink {
    fn id(&self) -> &str {
        &self.id
    }

    fn node_type(&self) -> &'static str {
        "sink/mqtt"
    }

    fn validate(&self) -> Result<()> {
        if self.broker.is_empty() {
            return Err(WaferError::Config(ConfigError::Message(
                "MQTT broker address cannot be empty".to_string(),
            )));
        }
        if self.topic.is_empty() {
            return Err(WaferError::Config(ConfigError::Message(
                "MQTT topic cannot be empty".to_string(),
            )));
        }
        if self.client_id.is_empty() {
            return Err(WaferError::Config(ConfigError::Message(
                "MQTT client_id cannot be empty".to_string(),
            )));
        }
        Ok(())
    }

    fn init(&mut self) -> Pin<Box<dyn Future<Output = Result<()>> + Send + '_>> {
        Box::pin(async move {
            let mut mqtt_options =
                MqttOptions::new(&self.client_id, &self.broker, self.port);
            mqtt_options.set_keep_alive(Duration::from_secs(30));

            let (client, eventloop) = AsyncClient::new(mqtt_options, 10);

            let handle = spawn_eventloop_task(eventloop);

            self.client = Some(client);
            self.eventloop_handle = Some(handle);
            Ok(())
        })
    }

    fn close(&mut self) -> Pin<Box<dyn Future<Output = Result<()>> + Send + '_>> {
        Box::pin(async move {
            if let Some(client) = self.client.take() {
                let _ = client.disconnect().await;
            }
            if let Some(handle) = self.eventloop_handle.take() {
                handle.abort();
            }
            Ok(())
        })
    }
}

fn spawn_eventloop_task(mut eventloop: EventLoop) -> JoinHandle<()> {
    tokio::spawn(async move {
        loop {
            if eventloop.poll().await.is_err() {
                tokio::time::sleep(Duration::from_millis(100)).await;
            }
        }
    })
}

impl Sink for MqttSink {
    fn collect(
        &mut self,
        envelope: RuntimeEnvelope,
    ) -> Pin<Box<dyn Future<Output = Result<()>> + Send + '_>> {
        Box::pin(async move {
            let client = self.client.as_ref().ok_or_else(|| WaferError::PluginInit {
                message: "MqttSink not initialized - call init() first".to_string(),
            })?;

            client
                .publish(&self.topic, self.qos, false, envelope.payload)
                .await
                .map_err(|e| {
                    WaferError::Io(std::io::Error::new(std::io::ErrorKind::Other, e.to_string()))
                })?;

            Ok(())
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_mqtt_sink_creation() {
        let sink = MqttSink::new("test-sink", "localhost", 1883, "test/topic", 1, "test-client");

        assert_eq!(sink.id(), "test-sink");
        assert_eq!(sink.node_type(), "sink/mqtt");
        assert_eq!(sink.broker, "localhost");
        assert_eq!(sink.port, 1883);
        assert_eq!(sink.topic, "test/topic");
        assert_eq!(sink.qos, QoS::AtLeastOnce);
        assert_eq!(sink.client_id, "test-client");
    }

    #[test]
    fn test_mqtt_sink_qos_mapping() {
        let sink0 = MqttSink::new("s", "b", 1883, "t", 0, "c");
        assert_eq!(sink0.qos, QoS::AtMostOnce);

        let sink1 = MqttSink::new("s", "b", 1883, "t", 1, "c");
        assert_eq!(sink1.qos, QoS::AtLeastOnce);

        let sink2 = MqttSink::new("s", "b", 1883, "t", 2, "c");
        assert_eq!(sink2.qos, QoS::ExactlyOnce);

        let sink3 = MqttSink::new("s", "b", 1883, "t", 99, "c");
        assert_eq!(sink3.qos, QoS::ExactlyOnce);
    }

    #[test]
    fn test_mqtt_sink_validate_empty_broker() {
        let sink = MqttSink::new("test-sink", "", 1883, "test/topic", 1, "test-client");
        assert!(sink.validate().is_err());
    }

    #[test]
    fn test_mqtt_sink_validate_empty_topic() {
        let sink = MqttSink::new("test-sink", "localhost", 1883, "", 1, "test-client");
        assert!(sink.validate().is_err());
    }

    #[test]
    fn test_mqtt_sink_validate_empty_client_id() {
        let sink = MqttSink::new("test-sink", "localhost", 1883, "test/topic", 1, "");
        assert!(sink.validate().is_err());
    }

    #[test]
    fn test_mqtt_sink_validate_success() {
        let sink = MqttSink::new("test-sink", "localhost", 1883, "test/topic", 1, "test-client");
        assert!(sink.validate().is_ok());
    }

    #[tokio::test]
    async fn test_mqtt_sink_collect_before_init() {
        let mut sink = MqttSink::new("test-sink", "localhost", 1883, "test/topic", 1, "test-client");
        let env = RuntimeEnvelope::from_string("test", "data");

        let result = sink.collect(env).await;
        assert!(result.is_err());
    }
}
